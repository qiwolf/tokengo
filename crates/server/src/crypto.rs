//! 密钥的 AES-256-GCM 加解密。
//!
//! 上游与可再次显示的用户 API Key 使用服务器主密钥加密保存；
//! 明文仅通过显式授权的显示接口返回，列表仍只提供掩码。
//!
//! 主密钥来源，按优先级：
//!   1. 环境变量 `TOKENGO_MASTER_KEY`（64 位十六进制 = 32 字节）
//!   2. 文件 `/etc/tokengo/master.key`（权限必须是 600）
//!
//! 主密钥丢了 = 已存的密钥全部解不开，只能重录。所以部署时要单独备份它。

use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;

pub const MASTER_KEY_ENV: &str = "TOKENGO_MASTER_KEY";
pub const MASTER_KEY_PATH: &str = "/etc/tokengo/master.key";
const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("主密钥必须是 64 位十六进制（32 字节），实际 {0} 位")]
    BadKeyLength(usize),
    #[error("主密钥不是合法的十六进制")]
    BadKeyEncoding,
    #[error("找不到主密钥：环境变量 {MASTER_KEY_ENV} 未设，且 {MASTER_KEY_PATH} 不存在")]
    MissingKey,
    #[error("主密钥文件权限是 {0:o}，必须是 600")]
    BadPermissions(u32),
    #[error("加密失败")]
    Encrypt,
    #[error("解密失败（主密钥不对，或数据被改过）")]
    Decrypt,
    #[error("密钥数据不是合法 UTF-8")]
    NotUtf8,
    #[error("读写主密钥文件失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 一把 AES-256-GCM 用的主密钥。
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    /// 从十六进制串构造。测试与部署都走这里。
    pub fn from_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let raw = hex::decode(hex_str.trim()).map_err(|_| CryptoError::BadKeyEncoding)?;
        if raw.len() != 32 {
            return Err(CryptoError::BadKeyLength(raw.len() * 2));
        }
        Ok(Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&raw)),
        })
    }

    /// 按环境变量 → 文件 的顺序找主密钥。
    pub fn from_env_or_file() -> Result<Self, CryptoError> {
        if let Ok(v) = std::env::var(MASTER_KEY_ENV) {
            if !v.trim().is_empty() {
                return Self::from_hex(&v);
            }
        }
        let p = Path::new(MASTER_KEY_PATH);
        if !p.exists() {
            return Err(CryptoError::MissingKey);
        }
        check_permissions(p)?;
        Self::from_hex(&std::fs::read_to_string(p)?.trim().to_owned())
    }

    /// 启动时用：环境变量 → 已有文件 → 首次生成并落盘。
    ///
    /// 首次生成会写 `master.key`（权限 600）。**写不进去就报错退出，不能默默用一把
    /// 临时的钥匙继续跑** —— 那样这次启动存进去的上游密钥，下次重启就再也解不开了。
    pub fn load_or_init(path: &Path) -> Result<Self, CryptoError> {
        if let Ok(v) = std::env::var(MASTER_KEY_ENV) {
            if !v.trim().is_empty() {
                return Self::from_hex(&v);
            }
        }
        if path.exists() {
            check_permissions(path)?;
            return Self::from_hex(std::fs::read_to_string(path)?.trim());
        }

        let (sb, hex_str) = Self::generate();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, &hex_str)?;
        set_owner_only(path)?;
        tracing::warn!(
            "首次启动，已生成主密钥 {}（权限 600）。**请单独备份它**：\
             丢了这把钥匙，已存的上游密钥全部解不开，只能重录。",
            path.display()
        );
        Ok(sb)
    }

    /// 生成一把新主密钥，返回（可用对象，十六进制串）。
    /// 十六进制串要立刻写进 600 的文件或环境变量，**不要打屏、不要入库**。
    pub fn generate() -> (Self, String) {
        let mut raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let hex_str = hex::encode(raw);
        (
            Self {
                cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&raw)),
            },
            hex_str,
        )
    }

    /// 加密。每次都用新的随机数（nonce），所以同一明文两次加密的密文不同。
    pub fn seal(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
            .map_err(|_| CryptoError::Encrypt)?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    pub fn open(&self, nonce: &[u8], cipher: &[u8]) -> Result<String, CryptoError> {
        if nonce.len() != NONCE_LEN {
            return Err(CryptoError::Decrypt);
        }
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), cipher)
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(pt).map_err(|_| CryptoError::NotUtf8)
    }
}

fn check_permissions(p: &Path) -> Result<(), CryptoError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(p)?.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(CryptoError::BadPermissions(mode));
    }
    Ok(())
}

fn set_owner_only(p: &Path) -> Result<(), CryptoError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(p)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(p, perms)?;
    Ok(())
}

/// 掩码：只露头尾，中间打点。日志、面板、错误信息里一律用这个，绝不回显明文。
pub fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n == 0 {
        return "(空)".into();
    }
    if n <= 8 {
        return "•".repeat(n);
    }
    let head: String = secret.chars().take(4).collect();
    let tail: String = secret.chars().skip(n - 4).collect();
    format!("{head}{}{tail}", "•".repeat(8.min(n - 8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const KEY_B: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

    #[test]
    fn round_trip() {
        let sb = SecretBox::from_hex(KEY_A).unwrap();
        let (nonce, ct) = sb.seal("sk-very-secret-key").unwrap();
        assert_eq!(sb.open(&nonce, &ct).unwrap(), "sk-very-secret-key");
        // 密文里不含明文
        assert!(!ct.windows(4).any(|w| w == b"sk-v"));
    }

    #[test]
    fn same_plaintext_encrypts_differently_each_time() {
        let sb = SecretBox::from_hex(KEY_A).unwrap();
        let (n1, c1) = sb.seal("same").unwrap();
        let (n2, c2) = sb.seal("same").unwrap();
        assert_ne!(n1, n2, "nonce 必须每次不同");
        assert_ne!(c1, c2, "密文必须每次不同");
    }

    #[test]
    fn wrong_master_key_fails_loudly() {
        let a = SecretBox::from_hex(KEY_A).unwrap();
        let b = SecretBox::from_hex(KEY_B).unwrap();
        let (nonce, ct) = a.seal("secret").unwrap();
        assert!(b.open(&nonce, &ct).is_err(), "换把钥匙必须解不开");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let sb = SecretBox::from_hex(KEY_A).unwrap();
        let (nonce, mut ct) = sb.seal("secret").unwrap();
        ct[0] ^= 0x01;
        assert!(sb.open(&nonce, &ct).is_err(), "改过一字节必须被发现");
    }

    #[test]
    fn rejects_bad_key_shapes() {
        assert!(matches!(
            SecretBox::from_hex("abcd"),
            Err(CryptoError::BadKeyLength(_))
        ));
        assert!(matches!(
            SecretBox::from_hex(&"zz".repeat(32)),
            Err(CryptoError::BadKeyEncoding)
        ));
    }

    #[test]
    fn mask_never_reveals_the_middle() {
        assert_eq!(mask(""), "(空)");
        assert_eq!(mask("short"), "•••••");
        let m = mask("sk-1234567890abcdef");
        assert!(m.starts_with("sk-1"), "{m}");
        assert!(m.ends_with("cdef"), "{m}");
        assert!(m.contains('•'));
        assert!(!m.contains("234567890"), "中间不能露出来: {m}");
    }

    /// 首次启动生成后必须能原样读回来 —— 读不回来意味着存的密钥下次重启就废了。
    #[test]
    fn load_or_init_creates_then_reuses_the_same_key() {
        use std::os::unix::fs::PermissionsExt;

        // 环境变量会盖过文件，测试里先确保它是空的
        std::env::remove_var(MASTER_KEY_ENV);

        let dir = std::env::temp_dir().join(format!("tokengo-keytest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("master.key");

        let first = SecretBox::load_or_init(&path).expect("首次应能生成");
        assert!(path.exists());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "主密钥文件权限必须是 600");

        let second = SecretBox::load_or_init(&path).expect("第二次应能读回");
        // 同一把钥匙：A 加的密 B 能解
        let (nonce, ct) = first.seal("sk-test").unwrap();
        assert_eq!(second.open(&nonce, &ct).unwrap(), "sk-test");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 权限被人改宽了必须拒绝启动，而不是照用。
    #[test]
    fn load_or_init_refuses_a_world_readable_key_file() {
        use std::os::unix::fs::PermissionsExt;

        std::env::remove_var(MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-keytest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("master.key");
        std::fs::write(&path, KEY_A).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let r = SecretBox::load_or_init(&path);
        assert!(
            matches!(r, Err(CryptoError::BadPermissions(0o644))),
            "644 的主密钥文件必须被拒绝"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
