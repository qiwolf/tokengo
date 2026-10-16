//! 运行配置。目前只吃环境变量；路径类都能被环境变量盖掉，方便测试与多实例。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub listen: String,
    /// SQLite 库文件。
    pub db_path: PathBuf,
    /// 主密钥文件（权限必须 600）。
    pub master_key_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:3160".into(),
            db_path: PathBuf::from("tokengo.db"),
            master_key_path: crate::crypto::MASTER_KEY_PATH.into(),
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let d = Self::default();
        Ok(Self {
            listen: std::env::var("TOKENGO_LISTEN").unwrap_or(d.listen),
            db_path: std::env::var("TOKENGO_DB")
                .map(PathBuf::from)
                .unwrap_or(d.db_path),
            master_key_path: std::env::var("TOKENGO_MASTER_KEY_FILE")
                .unwrap_or(d.master_key_path),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_loopback_only() {
        let c = Config::default();
        assert!(c.listen.starts_with("127.0.0.1:"), "默认只监听本机");
        assert_eq!(c.master_key_path, "/etc/tokengo/master.key");
    }

    #[test]
    fn env_overrides_win() {
        std::env::set_var("TOKENGO_LISTEN", "0.0.0.0:9999");
        std::env::set_var("TOKENGO_DB", "/tmp/x.db");
        std::env::set_var("TOKENGO_MASTER_KEY_FILE", "/tmp/k");
        let c = Config::from_env().unwrap();
        assert_eq!(c.listen, "0.0.0.0:9999");
        assert_eq!(c.db_path, PathBuf::from("/tmp/x.db"));
        assert_eq!(c.master_key_path, "/tmp/k");
        for k in ["TOKENGO_LISTEN", "TOKENGO_DB", "TOKENGO_MASTER_KEY_FILE"] {
            std::env::remove_var(k);
        }
    }
}

