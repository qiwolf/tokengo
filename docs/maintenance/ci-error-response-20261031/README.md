# main 错误响应测试时序调查（2026-10-31）

结论：原 CI 的四项失败尚未在 build-test 复现。已确认测试将正常路由、同步持久化和首次出站客户端初始化纳入了很短的行为断言窗口；现有证据支持加固测试时序，但不足以确认原运行器的具体资源故障，也没有形成修改服务端逻辑的依据。本补丁仅修改测试，不改生产超时、冷却或回退逻辑。

## 原始证据与范围

- [失败运行 40888253399](https://github.com/qiwolf/tokengo/actions/runs/40888253399)：main `22fb7897c955d482b14e5d3f7f3e6927aa9ffcb5`，`error_response` 7 通过、4 失败，耗时 4.43 秒。
- [成功运行 40884046534](https://github.com/qiwolf/tokengo/actions/runs/40884046534)：已发布源码 `5c6efa71da32803c6624f91515c01b323bc26c72` 的 Rust CI 成功。
- 两次日志的运行镜像均为 Ubuntu 24.04、`20261025.353.1`，Rust 均为 1.98.0；没有观察到这几个版本标识漂移。未记录当时 CPU 调度、磁盘延迟和逐请求时间线，不能据此排除运行资源差异。
- 本地 Git 对比确认两提交的 `crates/`、`Cargo.toml`、`Cargo.lock`、`.github/` 差异为空，19 个变更文件都是文档或验收收据。实时读取 GitHub main 时仍指向 `22fb789`。

| 原测试 | 失败点 | 判断边界 |
|---|---|---|
| `hanging_400_429_and_500_bodies_fail_over_and_release_capacity` | 原第 225 行，外层 500 ms 等待返回 `Elapsed` | 整个请求未在窗口内结束；不能单独认定错误正文无截止 |
| `heartbeat_only_200_has_precommit_deadline_and_falls_back` | 原第 291 行，504 而非 200 | 不能把“有备用渠道”等同于“剩余预算足够调用备用渠道” |
| `oversized_error_is_rejected_before_fallback` | 原第 243 行，504 而非 200 | 不直接证明 64 KiB 限制失效；预算可先于该断言耗尽 |
| `terminal_event_completes_without_waiting_for_connection_eof` | 原第 334 行，外层 100 ms 等待返回 `Elapsed` | 计时包含响应获取，未单独测量终止事件后的关闭 |

## 超时与回退的关系

以下行号对应基线 `22fb789` 的生产源码，本补丁未修改它们。

1. `crates/server/src/gateway.rs:1116` 在进入 `forward` 时记录请求起点。单次尝试另有 `t0`；它们包含该阶段中的同步工作。
2. `:1502–1509` 选择流式 `attempt_timeout_ms`；`:2070–2088` 在总预算用尽时返回 `gateway_timeout`。`:3816–3825` 将其编码为 HTTP 504、`upstream_timeout`。
3. `:2564–2566` 将请求响应头等待限制在单次超时与剩余总预算的最小值；`:2621–2634` 的预提交读取、`:3003–3015` 的错误正文读取又扣除已经耗费的尝试与请求时间。
4. `read_error`（`:4586` 起）同时限制 64 KiB、空闲时间和整体读取时间。持续滴流不能无限延长尝试。正文不完整或超限会记录 `channel_fault`、释放本次资源并尝试下一候选；下一候选仍必须有剩余请求预算。
5. 心跳不算有效输出，预提交失败可以在发出客户端内容之前回退。已经交付内容后的上游错误继续保留原流，不重放到备用渠道。
6. `prepare_stream` 在 `:4279–4282` 观察到终止事件时设 `prepared.eof=true`；`stream_passthrough` 在 `:4355–4368` 交付前缀后直接结束，无须等待上游连接文件结束信号（EOF）。

测试显式覆盖全局内存路由配置，使用新建独立数据库，没有额外的路由超时覆盖。原普通测试使用 80–150 ms 单次超时、600 ms 请求预算；终止测试只允许整个请求 100 ms。共享配置默认值是流式 30 秒、非流式 120 秒、总预算 180 秒（`crates/shared/src/routing.rs:36–45`），不能将这些默认值当成本测试的实际值，也不能当成当前生产配置的实测值。本次没有查询或修改生产配置。

两个具体的环境耗时敏感路径（源码推导，原 CI 耗时占比未知）：

- `upstream.rs:138` 经 `egress_client` 调用 `egress.rs` 的策略客户端缓存；首次缓存未命中会同步构建 HTTP 客户端。这发生在 `rb.send()` 的等待包装之前，随后仍从预提交/正文读取的剩余时间中扣除。各 Fixture 的缓存独立，初始化 `AppState` 不等于已预热该缓存。
- 数据库最初设为 `NORMAL`，随后 `AppState::init` 调用的 `billing_writer::replay` 在 `billing_writer.rs:340` 设为 `FULL`；`db/billing.rs:73` 也设为 `FULL`。请求中的同步落库和收尾可能等待磁盘同步。各 Fixture 使用独立数据库和状态，未发现跨 Fixture 共享数据库锁；其他测试的初始化仍可竞争机器资源。

这些路径解释了短窗口为什么脆弱，但不能证明原两次 504 分别在哪个阶段耗尽预算。没有逐请求正文与尝试账本，也不能仅凭状态码追溯到唯一的返回路径。没有证据支持把此次失败归为已确认的生产回归或已确认的运行器故障。

## 最小补丁

只修改 `crates/server/tests/error_response.rs`：

- 普通测试采用 1 秒单次截止、5 秒外层防挂死窗口、10 秒总预算。持续心跳/滴流仍每 10 ms 产生数据；错误续期会先触发 5 秒窗口，不能等到 10 秒总预算后侥幸通过。
- 专测总预算时，保持 `500 ms 预算 < 3 秒防挂死窗口 < 10 秒单次截止`，避免将单次超时当成总预算超时。
- 提取响应获取方法，终止测试先取得响应，再要求正文在 2 秒内结束；其流空闲超时是 10 秒，上游发送终止事件后永不结束连接。因此错误地等待连接关闭仍会失败。
- 原状态码、正文、回退、账本、容量释放和不重放断言全部保留。没有跳过测试、自动重试失败测试或修改数据库持久性保证。

这是测试时序加固补丁，不是已证实的服务端缺陷修复。增加时间余量不能保证任意过载主机均通过。

## 验证与复跑

执行主机是 `build-test`（Debian，Rust/Cargo 1.98.0），不等同于原 GitHub Ubuntu 运行器。每阶段先从 Mac 同步当前源码，核对逐文件 SHA256；只在 Mac 编辑。执行目录 `/srv/build/tokengo-ci-error-response-20261031/source`，依赖缓存独立于源码。详细命令、退出码及阶段版本见 [验证收据](validation.json)。

原样测试完成 9 轮：默认并发 3 轮、双核限制并固定 11 测试线程 3 轮、单核限制并固定 11 测试线程 3 轮，均为 11/11 通过。故原四项失败尚未复现，不能声称已证明并发压力就是原始原因。

补丁后默认并发 3 轮、恢复正常源码并清理重建后的双核/11 线程 3 轮，共 6 轮均为 11/11 通过。反向验证只在隔离构建副本中临时移除错误正文总截止、心跳预提交总截止、终止事件的 `eof` 标记：对应滴流、心跳、终止三项测试分别在约 5、5、2 秒失败（退出码 101，均为预期断言失败），说明放宽窗口后仍能捕捉这些实际逻辑缺陷。临时变异未写入主仓库；验证后从 Mac 重新同步正常源码。恢复源码时曾因保留旧文件时间戳而复用变异二进制，三轮 `patched-pressure` 结果已明确作废，原始失败日志仍保留；随后对本任务构建目录执行 `cargo clean -p tokengo-server` 并重建验证，避免将旧二进制结果当成当前源码结果。后续同步采用内容校验并不保留旧时间戳。

在已同步的 build-test 构建副本中复跑：

```sh
ssh build-test
hostname  # 必须为 build-test
cd /srv/build/tokengo-ci-error-response-20261031/source
export CARGO_HOME=/srv/cache/tokengo-cargo
export RUSTUP_HOME=/srv/cache/tokengo-rustup
export PATH=/srv/cache/tokengo-cargo/bin:$PATH
export CARGO_TARGET_DIR=/srv/build/tokengo-ci-error-response-20261031/target
cargo +1.98.0 test --locked -p tokengo-server --test error_response -- --nocapture
taskset -c 0,1 cargo +1.98.0 test --locked -p tokengo-server --test error_response -- --test-threads=11 --nocapture
```

Mac 上的本次同步清单、逐文件版本哈希、执行器、最小补丁及全部原始日志位于工作区 `review/ci-error-response-20261031/`。后续有源码变化必须重新同步，不能把上述命令指向的旧副本作为最新验证。测试只产生测试日志和临时数据库，没有新发行二进制或容器资产。首次构建因未同步编译内嵌的 `docs/references/.../sources.json` 失败，补齐该文件后才开始上述有效测试；准备失败日志另存 `results/setup/`，不计入测试轮次。

本次仅验证这 11 项相关测试及三项反向检查，没有重新运行整个 Rust CI、发行验收或生产请求。qiwolf.23 标签仍解析为 `5c6efa71da32803c6624f91515c01b323bc26c72`；发行资产和已通过的验收结论保持原状。
