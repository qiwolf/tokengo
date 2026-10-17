//! tokengo CLI。只做自用需要的几条命令，不做上游那 7 个文件的 CLI。

use tokengo_shared::{Router, RoutingConfig, BRAND, ORIGIN, VERSION};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | None => {
            println!("{BRAND} {VERSION}");
            println!("{ORIGIN}");
        }
        Some("routing") => {
            // 把默认路由参数打出来，方便对着 docs/03-路由策略.md 核
            let c = RoutingConfig::default();
            println!("重试次数（首次失败后）: {}", c.retry_times);
            println!("单条渠道最多尝试:         {}", c.max_attempts());
            println!("单次尝试超时:           {} ms", c.attempt_timeout_ms);
            println!("请求重试总预算:         {} ms", c.request_budget_ms);
            println!("回切检查间隔:           {} ms", c.reprobe_interval_ms);
            println!("后台探活:               {}", c.probe_enabled);
            // 走一遍状态机，确认默认参数下行为符合预期
            let channels = [tokengo_shared::ChannelId(1), tokengo_shared::ChannelId(2)];
            let mut r = Router::new(c);
            let now = 1_800_000_000_000i64;
            println!("\n模拟（渠道 #1 首选，渠道 #2 备选）：");
            println!("  首次请求顺序:        {:?}", r.attempt_order("auto", &channels, now));
            r.on_channel_exhausted("auto", tokengo_shared::ChannelId(1), &channels, now);
            println!("  首选失败后顺序:      {:?}", r.attempt_order("auto", &channels, now + 1000));
            println!("  10 分钟后顺序:       {:?}", r.attempt_order("auto", &channels, now + 600_000));
            r.on_success("auto", tokengo_shared::ChannelId(1), &channels, now + 600_001);
            println!("  探活成功后状态:      {:?}", r.state("auto"));
        }
        Some(other) => {
            eprintln!("未知子命令: {other}");
            eprintln!("可用: version | routing");
            std::process::exit(2);
        }
    }
    Ok(())
}
