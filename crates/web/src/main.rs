//! 面板的可执行入口。**它只干一件事：把界面挂到页面上。**
//!
//! 用 `trunk build --release` 编到 wasm32；直接 `cargo run` 是没意义的
//! （界面要浏览器环境），所以这里在非 wasm 目标下只打一句提示。

fn main() {
    // 用 cfg 而不是把整个入口包起来：这样 `cargo check --all-targets` 在本机
    // （x86）也能过，不需要先装 wasm 目标就能做静态检查。
    #[cfg(target_arch = "wasm32")]
    tokengo_web::start();

    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("tokengo 面板只能编到 wasm32：cd crates/web && trunk build --release");
}
