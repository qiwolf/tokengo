# 手机价格与滚动菜单修复

2026-11-14，根据用户两张Safari截图修复；本次唯一产品源码变化为crates/web/console-design.css。

价格表的手机卡片仍被640px最小宽度撑开，页面虽无横向滚动，字段与按钮却被容器裁切。现按手机宽度完整堆叠模型/来源、计价、参考值、状态与操作；长模型、来源名完整换行，按钮保留44px触控高度。菜单原为文档流中的static侧栏，滚动后打开时位于视口上方；现固定在56px顶栏下方，受100dvh约束、可独立滚动，关闭不改变页面滚动位置。桌面布局保留。

Mac编辑、Git管理；最新源码同步到build-test独立目录/srv/build/tokengo-mobile-layout-20261114并核对339文件哈希，Web release构建通过。与上一已验收工作树逐文件比较，仅CSS变化，所有Rust源完全相同；本次按CSS范围验证，不重复模型与账务测试。

实际编译WASM在Chromium及WebKit两引擎、320/390/430px、740×320横屏、1440×1000桌面、深浅色共20场景88项检查全部通过，页面/fixture错误0，96张截图取回Mac并目视390px浅色价格与菜单。逐字段检查18行、90字段和54操作的边界/scrollWidth，不再仅凭页面整体宽度判断完整显示。旧构建红测准确复现12项价格裁切与16项菜单离屏失败，桌面场景通过。

首轮红测缺少一条合成API，补齐后取得有效红测；首轮green暴露QA的同hash页面导航没有关闭菜单、短横屏只滚整行的隔离假设，已仅修QA并重跑，green-isolated为最终证据。未更改产品去迁就fixture。完整日志、脚本、几何数据、截图与Web归档位于Mac项目上级review/mobile-layout-20261114。

部署仅叠加Web静态资源，保留旧hashed资产及运行中的server/CLI，不重启模型服务，不写业务数据库。原子current切换，逐资源HTTP核对；失败只回旧Web链接。4项build-test隔离测试已验证成功保留、坏包/越界路径拒绝、资源失败回退且新业务记录保留、运行状态漂移不覆盖。已完成生产发布，回执见下节。

验证边界为Linux WebKit/Chromium实际渲染与生产资源字节一致性，不冒充真实iPhone Safari操作。历史共用知识《TokenGo 手机模型分类和筛选的同视口前后验收》（material_based/historical_needs_verification）仅作为同视口留证的方法参考；本轮事实均来自当前源码、红绿构建与部署读回。

## 生产回执

北京时间2026-11-14T11:16:52.153413+08:00完成Web-only更新，Web源码提交07262cb，现役静态资源路径/opt/tokengo/releases/web-mobile-07262cb6f6e9-969bc9bf。Web归档SHA256为969bc9bf859ad2495a9161a439cc13959eda930c72d2dd8e99f63886aee51318。公开HTML/JS/WASM/CSS共8项关键资源哈希与build-test产物逐一相符；服务内全部新旧静态资产已由manage.health逐文件核对。旧hashed资源保留供已打开页面继续加载，刷新页面取得新CSS。

模型服务PID985161、二进制SHA256 c23913c900196e7208d76674c039549d222a26ae46c3127d638ff7184a1ed2d1、完整环境及配置指纹前后完全一致。没有停API、关闭入口、数据库写入或重启模型服务；readyz返回ok。继承原server/CLI及模型规则，未推送或公开发版。

最终截图见evidence/webkit-390x900-light-token-row.png与webkit-390x900-light-scrolled-menu.png，分别显示完整字段和已滚动页面上的当前视口菜单。完整双引擎96张截图与逐字段几何报告仍在项目上级review/mobile-layout-20261114/evidence/green-isolated。
