//! 开发用小工具：对真库里的某个模型名模拟一次"首选渠道挂了 → 切到次选"，
//! 把切换状态写进数据库。
//!
//! 用途（阶段 2 代理还没做之前）：
//!   1. 验收"切换状态能落库、重启能接上"这条韧性要求；
//!   2. 手工造出"已切换"的局面，好在面板上看到切换计数与下次回切时间。
//!
//! 用法：`simulate_switch <库路径> [模型名] [次选渠道下标]`
//! 下标默认 1（即第二条渠道）。

use tokengo_server::{db, state::AppState, Config};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db_path = args.get(1).cloned().unwrap_or_else(|| "tokengo.db".into());
    let model = args.get(2).cloned().unwrap_or_else(|| "auto".into());
    let fallback_index: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);

    let cfg = Config {
        listen: "127.0.0.1:0".into(),
        db_path: db_path.into(),
        master_key_path: std::env::var("TOKENGO_MASTER_KEY_FILE")
            .unwrap_or_else(|_| "/etc/tokengo/master.key".into()),
        web_dir: None,
        log_dir: None,
    };
    let state = AppState::init(cfg)?;

    // `state.conn()` / `state.routing()` 返回的都是**不可重入**的锁守卫。
    // 一定要先出了花括号把它们放掉，再去调 persist_state —— 否则会自锁，
    // 现场表现是"命令卡住不动、没有任何输出"。
    let channels: Vec<tokengo_shared::ChannelId> = {
        let conn = state.conn();
        let ls = db::model_routes::channels_for(&conn, &model)?;
        if ls.is_empty() {
            anyhow::bail!("模型名 {model} 现在没有可用渠道（没人勾它 / 渠道停用了？）");
        }
        ls.into_iter().filter(|l| l.routable()).map(|l| l.id).collect()
    };
    if channels.len() < 2 {
        anyhow::bail!("模型名 {model} 只有 {} 条可用渠道，没法演示切换", channels.len());
    }

    let now = db::now_ms();
    let primary = channels[0];
    let target_index = fallback_index.min(channels.len() - 1);
    {
        let mut r = state.routing();
        r.attempt_order(&model, &channels, now);
        // 首选渠道重试用尽 → 切到下一条可用渠道
        r.on_channel_exhausted(&model, primary, &channels, now);
        // 想直接落到更靠后的那条，就把中间几条也依次"挂掉"
        for i in 1..target_index {
            r.on_channel_exhausted(&model, channels[i], &channels, now);
        }
    }
    state.persist_state(&model)?;

    let st = state.routing().state(&model).cloned().unwrap_or_default();
    println!("模型名 {model}：");
    println!(
        "  渠道顺序（=优先级）  ：{:?}",
        channels.iter().map(|l| l.0).collect::<Vec<_>>()
    );
    println!("  首选渠道             ：{primary}");
    println!(
        "  模拟后当前渠道       ：{}",
        st.active
            .map(|l| l.0.to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!("  切换次数           ：{}", st.switch_count);
    println!(
        "  下次回切检查点     ：{}",
        st.next_probe_at_ms
            .map(|t| format!("{t}（约 {} 分钟后）", (t - now) / 60_000))
            .unwrap_or_else(|| "无".into())
    );
    Ok(())
}
