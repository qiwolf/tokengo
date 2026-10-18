//! 运行配置。目前只吃环境变量；路径类都能被环境变量盖掉，方便测试与多实例。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub listen: String,
    /// SQLite 库文件。
    pub db_path: PathBuf,
    /// 主密钥文件（权限必须 600）。
    pub master_key_path: String,
    /// 面板构建产物目录（`trunk build --release` 的输出：index.html + pkg/）。
    /// `None` = 没找到 —— 这时面板路径**如实说没构建**，而不是回一个空壳页面。
    pub web_dir: Option<PathBuf>,
    /// 日志目录。`None` = 只进内存缓冲与 journald，不落文件（测试环境）。
    pub log_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:3160".into(),
            db_path: PathBuf::from("tokengo.db"),
            master_key_path: crate::crypto::MASTER_KEY_PATH.into(),
            web_dir: None,
            log_dir: None,
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let d = Self::default();
        let web_dir = match std::env::var("TOKENGO_WEB_DIR") {
            // 空串最容易在部署脚本里出现（`TOKENGO_WEB_DIR=$没定义的变量`），
            // 那种情况要退回自动查找，不能把当前目录当面板目录。
            Ok(v) if !v.trim().is_empty() => Some(PathBuf::from(v)),
            _ => find_web_dir(),
        };
        Ok(Self {
            listen: std::env::var("TOKENGO_LISTEN").unwrap_or(d.listen),
            db_path: std::env::var("TOKENGO_DB")
                .map(PathBuf::from)
                .unwrap_or(d.db_path),
            master_key_path: std::env::var("TOKENGO_MASTER_KEY_FILE")
                .unwrap_or(d.master_key_path),
            web_dir,
            // 日志目录：默认 ./logs（部署时给绝对路径，别跟着工作目录跑）
            log_dir: std::env::var("TOKENGO_LOG_DIR")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from)
                .or(d.log_dir),
        })
    }

    /// 面板产物目录；`None` = 没构建。
    pub fn web_dir(&self) -> Option<&Path> {
        self.web_dir.as_deref()
    }
}

/// 自动找面板产物。**顺序即优先级**，取第一个含 `index.html` 的目录：
///
/// 1. 二进制旁边的 `web/`（部署时把 dist 拷成 `web/` 放在二进制旁）
/// 2. 工作目录下的 `crates/web/dist`（在仓库里直接跑）
/// 3. 工作目录下的 `dist`
fn find_web_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web"));
        }
    }
    candidates.push(PathBuf::from("crates/web/dist"));
    candidates.push(PathBuf::from("dist"));

    candidates.into_iter().find(|p| p.join("index.html").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 环境变量与工作目录是**进程级**状态，而 cargo 的测试是同进程多线程并行跑。
    /// 一个用例改了 cwd / 环境变量，另一个用例正好在这中间读，就会随机红。
    /// 所以凡是动这两样的用例都要先拿这把锁（踩过：`env_overrides_win` 偶发
    /// `web_dir` 变成 None，真相是另一个用例把 cwd 换走了，不是代码错）。
    static PROCESS_STATE: Mutex<()> = Mutex::new(());

    fn lock_process_state() -> std::sync::MutexGuard<'static, ()> {
        PROCESS_STATE.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn defaults_are_loopback_only() {
        let c = Config::default();
        assert!(c.listen.starts_with("127.0.0.1:"), "默认只监听本机");
        assert_eq!(c.master_key_path, "/etc/tokengo/master.key");
    }

    #[test]
    fn env_overrides_win() {
        let _g = lock_process_state();
        std::env::set_var("TOKENGO_LISTEN", "0.0.0.0:9999");
        std::env::set_var("TOKENGO_DB", "/tmp/x.db");
        std::env::set_var("TOKENGO_MASTER_KEY_FILE", "/tmp/k");
        std::env::set_var("TOKENGO_WEB_DIR", "/tmp/panel-dist");
        let c = Config::from_env().unwrap();
        assert_eq!(c.listen, "0.0.0.0:9999");
        assert_eq!(c.db_path, PathBuf::from("/tmp/x.db"));
        assert_eq!(c.master_key_path, "/tmp/k");
        assert_eq!(c.web_dir, Some(PathBuf::from("/tmp/panel-dist")));
        for k in [
            "TOKENGO_LISTEN",
            "TOKENGO_DB",
            "TOKENGO_MASTER_KEY_FILE",
            "TOKENGO_WEB_DIR",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn empty_web_dir_env_is_treated_as_unset() {
        let _g = lock_process_state();
        std::env::set_var("TOKENGO_WEB_DIR", "   ");
        let c = Config::from_env().unwrap();
        assert_ne!(c.web_dir, Some(PathBuf::from("   ")));
        std::env::remove_var("TOKENGO_WEB_DIR");
    }

    #[test]
    fn find_web_dir_requires_an_index_html() {
        let _g = lock_process_state();
        // 产物目录里必须有 index.html：否则面板会打开一个空壳，看着像坏了。
        let base = std::env::temp_dir().join(format!("tokengo-webdir-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("dist")).unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&base).unwrap();
        assert!(find_web_dir().is_none(), "没有 index.html 就不该被选中");
        std::fs::write(base.join("dist").join("index.html"), "<html></html>").unwrap();
        assert_eq!(find_web_dir(), Some(PathBuf::from("dist")));
        std::env::set_current_dir(old).unwrap();
        std::fs::remove_dir_all(&base).ok();
    }
}
