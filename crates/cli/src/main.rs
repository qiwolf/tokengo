//! tokengo CLI（对应上游 `cli/`，2,870 行 / 7 个文件）。阶段 8 实装。

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | None => {
            println!("tokengo {}", tokengo_shared::VERSION);
            println!("{}", tokengo_shared::UPSTREAM);
            println!("平台数: {}", tokengo_shared::Platform::ALL.len());
            Ok(())
        }
        Some("platforms") => {
            for p in tokengo_shared::Platform::ALL {
                println!("{p}");
            }
            Ok(())
        }
        Some(other) => {
            eprintln!("未知子命令: {other}");
            eprintln!("可用: version | platforms");
            std::process::exit(2);
        }
    }
}
