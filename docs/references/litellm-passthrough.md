# LiteLLM 透传实现参考

本功能研究的公开源码固定为 BerriAI/litellm commit `045fb7ba84803e3265462c7988ecfaab35351860`。参考对象为通用 HTTP 路由注册、子路径拼接、静态 header 与响应流生命周期；未引入 Python 运行时或供应商协议转换代码。

- [通用透传及路由注册](https://github.com/BerriAI/litellm/blob/045fb7ba84803e3265462c7988ecfaab35351860/litellm/proxy/pass_through_endpoints/pass_through_endpoints.py)
- [许可证](https://github.com/BerriAI/litellm/blob/045fb7ba84803e3265462c7988ecfaab35351860/LICENSE)：所查 `enterprise/` 外代码为 MIT，Copyright (c) 2023 Berri AI。

Tokengo 使用独立 Rust transport，配置进入既有 SQLite 与 Leptos 面板，权限和费用进入既有 VKey / admission / 持久化账本。不会照搬 LiteLLM 的业务 JSON 解析、响应重写或厂商专用逻辑。

关键差异：Tokengo 的 `prefix` 是字面前缀，最长匹配，默认剥离；不是 FastAPI 的 `*` 通配表达式。已核实 LiteLLM 1.100.0 的 `include_subpath` 默认为 false，不能把配置中的 `/face/*` 自然解释为已经注册 `/face/represent`。

本实现保留实体正文及 Content-Encoding 原字节，过滤 hop-by-hop 与 Connection 指名字段并保留重复端到端响应字段；不承诺保留 HTTP 传输分块边界。上传和下载受背压及总期限控制，默认零重试，禁止自动重定向。
