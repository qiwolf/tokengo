# 反向代理与真实来源 IP

请求详情中的来源 IP 取决于接入链路是否保留了调用方地址。`10.x` 等私网地址可能是合法的内网调用，也可能是反代、隧道或源地址转换后的连接地址，不能只凭地址类型认定请求异常。

TokenGo 当前先取 `X-Forwarded-For` 的首项；该首项不可用或为空时，再取 `X-Real-IP`；没有可用头时回退到直接连接的来源地址。这是读取请求头的逻辑，并未在应用中新增可信代理地址列表。因此应将应用监听端口限制在回环或受控网络，只允许可信反代接入，不能把客户端可伪造的请求头直接当作可信身份。

## 单层入口

直接接收客户端连接的 Nginx 应覆盖客户端传入的来源头。以下片段放在现有 `location` 中，保留原来的 HTTPS、上游、流式超时和缓冲设置：

```nginx
proxy_set_header X-Real-IP $remote_addr;
proxy_set_header X-Forwarded-For $remote_addr;
```

这会记录入口看到的实际连接来源。若入口前面还有 CDN、负载均衡或另一层代理，需要先按该服务的可信地址和来源头规则恢复地址；不能在所有来源上无条件信任任意 `X-Forwarded-For`。

## VPS → 隧道 → 内网 Nginx → TokenGo

外层 VPS 仍用上面的配置覆盖两个来源头。内层如果也直接用原始 `$remote_addr` 覆盖它们，就会把 VPS 的隧道地址写入 TokenGo。

内层可用 Nginx realip 模块，先从可信 VPS 的 `X-Real-IP` 恢复地址，再通过原有两个 `proxy_set_header` 传给应用：

```nginx
server {
    # Example only: replace with the exact VPS source seen by this Nginx.
    set_real_ip_from 192.0.2.10/32;
    real_ip_header X-Real-IP;
    real_ip_recursive off;

    # Keep the existing listen, TLS and location configuration.
    location / {
        proxy_pass http://127.0.0.1:3160;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $remote_addr;
        # Keep existing Host, Authorization, timeout and SSE settings.
    }
}
```

`192.0.2.10` 是文档示例地址，不是可直接部署的默认值。必须换成内层实际看到、且已确认只属于受控 VPS 的连接源地址；IPv6 使用对应的 `/128`。不要改成 `0.0.0.0/0`、`::/0` 或无依据的整个内网网段。此方案依赖 VPS 始终覆盖 `X-Real-IP`，并限制谁能从该可信地址建立连接。

Nginx 须包含 `http_realip_module`。配置含义见 [Nginx realip 官方文档](https://nginx.org/en/docs/http/ngx_http_realip_module.html)：只有匹配 `set_real_ip_from` 的连接才使用指定头替代地址。若地址被前置 NAT 合并，应先核实实际拓扑，不能把一个共享来源地址当成所有流量都可信的依据。

## 上线与核对

先保存活动配置和回滚副本，用 `nginx -t` 检查，再平滑重载。用带唯一标记的公网健康请求对照外层与内层日志，核对恢复的地址一致；同时故意传入假的 `X-Real-IP` 和 `X-Forwarded-For`，验证入口覆盖生效，再从不可信来源直连验证该头不会被采用。最后检查服务健康和正常业务。

健康检查与日志一致只能证明该请求的接入链路；模型推理、流式响应和请求数据库中的来源记录应按需要另行验证。历史请求保存的是当时记录，反代修复不会改写历史来源 IP；内网直连请求仍可能合理地显示内网 IP。

这项修复属于部署配置。下载或升级 TokenGo 二进制、容器镜像不会自动修改现有 Nginx，也不会自动建立可信代理规则。2026-11-02 的现场修复和验收边界见[237 运维记录](maintenance/source-ip-20261102/README.md)。
