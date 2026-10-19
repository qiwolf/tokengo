//! Bounded multi-key authentication cache. No plaintext is retained.
//!
//! AppState holds the database lock through cache lookup, validation and fill;
//! mutation handlers hold that same lock through deletion and cache clearing.
//! Lock order is always DB -> cache. Two independent keyed hashes identify entries.

use std::collections::{hash_map::RandomState, HashMap};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;

/// Cache entries expire after five minutes; credential mutations clear them immediately.
pub const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

struct Cached {
    digest: (u64, u64),
    at_ms: i64,
}

pub struct GatewayKeyCache {
    inner: Mutex<HashMap<(u64, u64), Cached>>,
    hashers: [RandomState; 2],
}

impl Default for GatewayKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayKeyCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            hashers: [RandomState::new(), RandomState::new()],
        }
    }

    fn digest(&self, key: &str) -> (u64, u64) {
        let digest = |seed: &RandomState| {
            let mut h = seed.build_hasher();
            key.hash(&mut h);
            h.finish()
        };
        (digest(&self.hashers[0]), digest(&self.hashers[1]))
    }

    /// 缓存命中？命中就说明**曾经**真校验通过过，且没过期。
    pub fn hit(&self, key: &str, now_ms: i64) -> bool {
        if key.trim().is_empty() {
            return false;
        }
        let d = self.digest(key);
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(&d) {
            Some(c) => {
                c.digest == d
                    && now_ms >= c.at_ms
                    && now_ms - c.at_ms < CACHE_TTL.as_millis() as i64
            }
            None => false,
        }
    }

    /// 真校验通过后调它记账。
    pub fn remember(&self, key: &str, now_ms: i64) {
        let d = self.digest(key);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, c| now_ms >= c.at_ms && now_ms - c.at_ms < CACHE_TTL.as_millis() as i64);
        if g.len() >= 1024 {
            g.clear();
        }
        g.insert(
            d,
            Cached {
                digest: d,
                at_ms: now_ms,
            },
        );
    }

    /// 换了 key / 清了 key 就把它忘掉。**换 key 必须调**，否则旧 key 还能用最多 5 分钟。
    pub fn forget(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_only_the_key_that_passed() {
        let c = GatewayKeyCache::new();
        assert!(!c.hit("sk-1", 1000), "还没校验过就不算命中");
        c.remember("sk-1", 1000);
        c.remember("sibling", 1000);
        assert!(c.hit("sibling", 1000));
        assert!(c.hit("sk-1", 1000));
        assert!(!c.hit("sk-2", 1000), "别的 key 不能蹭");
        assert!(!c.hit("", 1000));
    }

    #[test]
    fn the_cache_expires() {
        let c = GatewayKeyCache::new();
        c.remember("sk-1", 1000);
        assert!(c.hit("sk-1", 1000 + 299_000));
        assert!(!c.hit("sk-1", 1000 + 300_001), "过期要重新真校验");
    }

    /// 换 key 之后旧 key 立刻失效 —— 缓存忘了清就是安全问题。
    #[test]
    fn forgetting_kills_the_fast_path() {
        let c = GatewayKeyCache::new();
        c.remember("old", 1000);
        assert!(c.hit("old", 1000));
        c.forget();
        assert!(!c.hit("old", 1000));
    }

    /// 内存里不留明文：拿到的只是一把键控哈希。
    #[test]
    fn digest_is_keyed_and_not_the_plaintext() {
        let c = GatewayKeyCache::new();
        assert_ne!(c.digest("sk-abc"), (0, 0));
        let other = GatewayKeyCache::new();
        // 每个进程一份随机种子，两个实例的哈希不同（内存转储拿不到可比对的值）
        let a = c.digest("sk-abc");
        let b = other.digest("sk-abc");
        assert_ne!(a, b);
    }
}
