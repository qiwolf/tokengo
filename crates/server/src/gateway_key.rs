//! 对外统一 key 的校验，带一层进程内快路径缓存。
//!
//! 库里只存 argon2 慢哈希（这是对的，口令类东西要抗猜解）。但**每个请求**都跑一遍
//! argon2 代价很大：默认参数每次要 19 MB 内存 + 几十毫秒 CPU，落在请求路径上等于
//! 给每一次调用平白加几十毫秒，并发一上来还会把 tokio 的工作线程占满。
//!
//! 所以：第一次校验照旧跑 argon2（结果对就是对的，安全边界没变），把"这个 key 通过了"
//! 这件事记在内存里，之后按**键控哈希**（每个进程一份随机种子的 SipHash）比对即可。
//! 内存里不留明文 key；缓存有 TTL，过一会儿重新真校验一遍，防的是有人绕过面板直接改库。

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;

/// 缓存有效期：5 分钟。到点重新走一次 argon2。
pub const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

struct Cached {
    digest: u64,
    at_ms: i64,
}

pub struct GatewayKeyCache {
    inner: Mutex<Option<Cached>>,
    hasher: RandomState,
}

impl Default for GatewayKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayKeyCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            hasher: RandomState::new(),
        }
    }

    fn digest(&self, key: &str) -> u64 {
        let mut h = self.hasher.build_hasher();
        key.hash(&mut h);
        h.finish()
    }

    /// 缓存命中？命中就说明**曾经**真校验通过过，且没过期。
    pub fn hit(&self, key: &str, now_ms: i64) -> bool {
        if key.trim().is_empty() {
            return false;
        }
        let d = self.digest(key);
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &*g {
            Some(c) => {
                c.digest == d && now_ms - c.at_ms < CACHE_TTL.as_millis() as i64
            }
            None => false,
        }
    }

    /// 真校验通过后调它记账。
    pub fn remember(&self, key: &str, now_ms: i64) {
        let d = self.digest(key);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(Cached { digest: d, at_ms: now_ms });
    }

    /// 换了 key / 清了 key 就把它忘掉。**换 key 必须调**，否则旧 key 还能用最多 5 分钟。
    pub fn forget(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *g = None;
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
        assert_ne!(c.digest("sk-abc"), 0);
        let other = GatewayKeyCache::new();
        // 每个进程一份随机种子，两个实例的哈希不同（内存转储拿不到可比对的值）
        let a = c.digest("sk-abc");
        let b = other.digest("sk-abc");
        assert_ne!(a, b);
    }
}
