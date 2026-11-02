# 237 来源 IP 修复（2026-11-02）

已在 237 的 `/etc/nginx/sites-available/tokengo` 上线。仅修改内网 Nginx，VPS、应用程序及数据库未改动。

## 原因与修复

VPS 将客户端地址写入 `X-Real-IP` 和 `X-Forwarded-For`，经 WireGuard 以 `10.10.1.3` 连接 237。237 原先用 `$remote_addr` 覆盖两个头，导致 TokenGo 记录隧道地址。

新增 `set_real_ip_from 10.10.1.3/32`、`real_ip_header X-Real-IP`、`real_ip_recursive off`。仅从该 VPS 接收替代来源地址；原有两个代理头继续使用处理后的 `$remote_addr`，因此不盲信任直连请求携带的来源头。此信任关系依赖 VPS 始终覆盖 X-Real-IP；VPS 隧道来源地址变更时须同步核验。

## 验收证据（observed，derived）

- 修复前：带唯一标记的公网 `/readyz` 请求在 237 记录为 `10.10.1.3`。
- 目标主机 `nginx -t` 通过，平滑 reload 返回 0；Nginx 与 TokenGo 均 active。
- 修复后：带伪造 `X-Real-IP`、`X-Forwarded-For` 的公网请求返回 200；VPS 与 237 的同标记日志来源地址完全一致，为公网地址，未采用伪造值。
- 不可信来源测试：从 237 本机直连并伪造两个头，返回 200，237 仍记录实际来源 `127.0.0.1`。
- `proxy_set_header` 使用恢复后的 `$remote_addr`，故转发给 TokenGo 的来源头随之修正（配置推导）。本次未为验收生成付费模型请求，未独立取得新增公网模型请求的数据库记录；不把健康检查写成模型推理验收。
- 线上 Nginx 配置必须在真实接入链路检查，因此此部分直接在 VPS/237 执行；没有项目构建、编译或测试，未使用 build-test。

本地原始收据：`review/source-ip-20261102/evidence/`（Mac 项目根目录下），包括修改前后日志、配置哈希、部署回执及脱敏汇总 `acceptance.json`。提交副本不包含客户端公网 IP 或凭据。

## 回滚

237 上备份位于 `/var/backups/tokengo/source-ip-20261102T083110Z/`。将其中 `nginx-before.conf` 恢复到 `/etc/nginx/sites-available/tokengo`，执行 `nginx -t` 成功后 `systemctl reload nginx`。回滚会恢复旧的来源 IP 行为。

历史请求保留原值；本次没有修改历史数据。
