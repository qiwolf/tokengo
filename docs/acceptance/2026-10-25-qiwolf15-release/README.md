# qiwolf.15 发布记录

2026-10-25（北京时间）。本地main同步远端后，整合已验收UI及文档，按精确提交完成构建、合并测试并正式发布。

## 发行来源

- 正式版：[v0.34.0-qiwolf.15](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.15)，发行提交 `c3ecd31250f061f781970b45217cf31743f41e7d`。
- [main CI](https://github.com/qiwolf/tokengo/actions/runs/40109365222)与[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40109975693)均成功。Rust1182通过、0失败、3项按既有约定忽略，WASM检查通过。
- build-test本地同提交133项前端测试、Linux原生/WASM构建、配套原生包与容器生命周期测试均通过；与workspace覆盖重叠，不累加。
- UI已完成284项有效浏览器检查，覆盖10个页面及14个关键操作界面、明暗主题和桌面/窄屏。用户在既有实例复核通过。
- Cargo0.34.0/schema40保持不变，没有新增业务接口、权限或数据库迁移变更。

## 公开分发

GitHub发布工作流完成同源原生包、WASM、容器构建及运行验证，发布9个附件和GHCR镜像。附件与校验文件可从Release下载。

- 原生包：`tokengo-c3ecd31250f0-linux-x86_64.tar.gz`。
- GitHub记录的原生包SHA256：`ad98027d2df25f468a31679c724983073b093f645487fb545acf601ad9e59aa6`。
- 发布工作流提供的不可变镜像：`ghcr.io/qiwolf/tokengo@sha256:63b9b32289cf1482867f74889cf6e914aa0ddbb06934e74beeef211eedc67083`。
- 精确来源、CI链接及分发摘要见publication.json。

## 部署与文档

现有实例此前仅更新前端，用户已确认；本次公开发布没有再次替换该实例的运行服务。使用原生包或容器升级时，按部署文档保存数据库、主密钥、配置与旧版本。

标签与下载包文档为构建时快照，后补发布记录维护于main与Release页面；未移动标签或重打既有附件。本轮构建、测试日志及交付产物已回收到Mac项目任务目录。
