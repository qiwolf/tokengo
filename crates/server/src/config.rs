//! 运行配置。阶段 0 只吃环境变量，阶段 1 换成正式配置层（文件 + 环境变量 + 数据库覆盖）。

pub struct Config {
    pub listen: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            listen: std::env::var("TOKENGO_LISTEN").unwrap_or_else(|_| "127.0.0.1:3160".into()),
        })
    }
}
