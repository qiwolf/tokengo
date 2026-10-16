//! tokengo 面板（对应上游 `client/`：18 个页面 + 83 个通用组件，24,221 行）。
//!
//! **阶段 7 实装，框架选 Leptos（编 WASM）。** 现在是占位，只保证工作区能本机构建。
//!
//! 为什么先把坑留在这里而不是等到阶段 7 再说：Leptos 在本机是**首次引入**，
//! 需要装 `wasm32-unknown-unknown` 目标与 `trunk`，还要确认本机能产出 wasm。
//! 这个风险要在写服务端之前先证掉，不能等写了三周服务端才发现面板路走不通。
//!
//! 面板要覆盖的上游页面（`client/src/pages/`，逐个对应）：
//!   见 `docs/01-上游清点.md` 第六节。

/// 面板入口在阶段 7 由 Leptos 提供。现在只暴露品牌标识，供服务端页脚复用。
pub const BRAND: &str = "tokengo";

/// 面板需要覆盖的页面清单（上游 `client/src/pages/` 的移植对照表，阶段 0 先立着）。
pub const PAGES: &[&str] = &[];

#[cfg(test)]
mod tests {
    #[test]
    fn brand_is_set() {
        assert_eq!(super::BRAND, "tokengo");
    }
}
