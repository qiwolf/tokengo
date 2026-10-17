//! 库表定义。迁移按版本号顺序执行，已执行的记在 `schema_migrations` 里。
//!
//! 设计取舍（对应 `docs/02-按需功能清单.md`）：
//! - 现网 FreeLLMAPI 33 张表，本机真在用的只有几张；这里按实际需要重建，共 11 张。
//! - **免费模型目录相关的表一张都没有**（models / embedding_models / quirks /
//!   catalog_* / media_models …）——用户明确不要，且那正是内存与库体积持续增长的来源。
//! - `channel_headers` 是上游没有的增项：按渠道配自定义请求头。
//!
//! **术语**：2026-10-17 起「腿」改叫「渠道」（channel）。v1/v2 里的 SQL 与注释是当时的
//! 原文，按「迁移只追加」的规矩**原样保留**（里面出现的 legs / leg_id 是历史名字，别改，
//! 也别用 `scratch/rename_leg_to_channel.py` 刷这一段）；v3 负责把新库和老库都改到新名字。

/// 迁移列表。**只追加，不改已发布的条目**——改历史条目会让已部署的库对不上。
pub const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        r#"
-- 腿：一条上游通道
CREATE TABLE legs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    base_url    TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- 腿的凭据，加密存（明文绝不落库）
CREATE TABLE leg_secrets (
    leg_id  INTEGER PRIMARY KEY REFERENCES legs(id) ON DELETE CASCADE,
    nonce   BLOB    NOT NULL,
    cipher  BLOB    NOT NULL
);

-- 腿的自定义请求头。上游 FreeLLMAPI 没有这个能力 —— 本项目主动增项。
-- value 支持 {{uuid}} / {{timestamp_ms}} 模板，逐请求求值。
CREATE TABLE leg_headers (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    leg_id  INTEGER NOT NULL REFERENCES legs(id) ON DELETE CASCADE,
    name    TEXT    NOT NULL,
    value   TEXT    NOT NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    UNIQUE (leg_id, name)
);

-- 每条腿认的模型名。同一个对外模型名在不同腿上的上游名字往往不一样。
CREATE TABLE leg_models (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    leg_id         INTEGER NOT NULL REFERENCES legs(id) ON DELETE CASCADE,
    model_name     TEXT    NOT NULL,
    upstream_model TEXT,
    UNIQUE (leg_id, model_name)
);

-- 模型组：一个对外模型名对应一张有序腿列表
CREATE TABLE model_groups (
    name    TEXT    PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE model_group_legs (
    group_name TEXT    NOT NULL REFERENCES model_groups(name) ON DELETE CASCADE,
    leg_id     INTEGER NOT NULL REFERENCES legs(id)         ON DELETE CASCADE,
    ordinal    INTEGER NOT NULL,
    PRIMARY KEY (group_name, leg_id)
);

-- 路由状态：当前腿 / 下次回切时间 / 切换次数
CREATE TABLE routing_state (
    group_name        TEXT PRIMARY KEY,
    active_leg_id     INTEGER,
    next_probe_at_ms  INTEGER,
    switch_count      INTEGER NOT NULL DEFAULT 0,
    last_switch_at_ms INTEGER
);

-- 记账（请求级）
CREATE TABLE requests (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms         INTEGER NOT NULL,
    endpoint      TEXT    NOT NULL,
    model         TEXT    NOT NULL,
    status        TEXT    NOT NULL,
    http_status   INTEGER,
    leg_id        INTEGER,
    latency_ms    INTEGER NOT NULL DEFAULT 0,
    first_byte_ms INTEGER,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    streamed      INTEGER NOT NULL DEFAULT 0,
    client_ip     TEXT,
    error         TEXT
);
CREATE INDEX idx_requests_ts  ON requests (ts_ms DESC);
CREATE INDEX idx_requests_leg ON requests (leg_id, ts_ms DESC);

-- 记账（尝试级）：一次请求在第几条腿上第几次尝试、结果如何
CREATE TABLE attempts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id  INTEGER NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    leg_id      INTEGER NOT NULL,
    outcome     TEXT    NOT NULL,
    http_status INTEGER,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    error       TEXT
);
CREATE INDEX idx_attempts_request ON attempts (request_id, ordinal);

-- 设置（路由参数、统一对外 key 等）
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 面板会话
CREATE TABLE panel_sessions (
    token         TEXT    PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    client_ip     TEXT
);
CREATE INDEX idx_panel_sessions_expiry ON panel_sessions (expires_at_ms);
"#,
    ),
    (
        2,
        r#"
-- 视觉能力标记（阶段 4.1）：这个模型名在这条腿上能不能吃图片输入。
--
-- 为什么要它：客户端（Hermes、面板的假上游测试）会把图发给 `auto`，而
-- `auto` 后面那串腿里有的落在纯文本模型上 —— 上游只会回一个含糊的 400
-- 或干脆把图忽略掉，用户看不出来发生了什么。有了这个标记，带图的请求
-- 只走「标了能看图」的腿；一条都没有就 422 明说，而不是把图悄悄丢掉。
--
-- 默认 0：**没标 = 不敢保证**。宁可先说"这组里没有能看图的腿"，也不猜。
ALTER TABLE leg_models ADD COLUMN vision INTEGER NOT NULL DEFAULT 0;
"#,
    ),
    (
        3,
        r#"
-- 术语改名：腿（leg）→ 渠道（channel）。
--
-- 为什么要有这条迁移：v1 的条目按规范不许改（改历史条目会让已部署的库对不上），
-- 于是改成「先建旧名、再改名」—— 新库与旧库走的是同一条路径，最终 schema 一致。
-- 代码里也是同口径：Leg→Channel、leg_id→channel_id、表 legs→channels。
--
-- SQLite 3.25+ 的 RENAME TABLE 会跟着更新外键引用（这里 foreign_keys=ON，
-- model_group_channels 等子表指向 channels 的引用会自动跟上）。
ALTER TABLE legs             RENAME TO channels;
ALTER TABLE leg_secrets      RENAME TO channel_secrets;
ALTER TABLE leg_headers      RENAME TO channel_headers;
ALTER TABLE leg_models       RENAME TO channel_models;
ALTER TABLE model_group_legs RENAME TO model_group_channels;

ALTER TABLE requests      RENAME COLUMN leg_id         TO channel_id;
ALTER TABLE attempts      RENAME COLUMN leg_id         TO channel_id;
ALTER TABLE routing_state RENAME COLUMN active_leg_id  TO active_channel_id;
-- 子表的列名也要换：v1 里建的是 leg_id
ALTER TABLE channel_secrets      RENAME COLUMN leg_id TO channel_id;
ALTER TABLE channel_headers      RENAME COLUMN leg_id TO channel_id;
ALTER TABLE channel_models       RENAME COLUMN leg_id TO channel_id;
ALTER TABLE model_group_channels RENAME COLUMN leg_id TO channel_id;

-- 索引名也换：列改名后索引定义会跟着变，但索引自己的名字不会。
DROP INDEX IF EXISTS idx_requests_leg;
CREATE INDEX idx_requests_channel ON requests (channel_id, ts_ms DESC);
"#,
    ),
    (
        4,
        r#"
-- 渠道级请求/响应适配（2026-10-17）：把现网两条 python 中转（ocgo-relay 3157、
-- cline-relay 3158）的活接过来，这样它们才能退役。下面两项都不是猜出来的，是现网
-- 4 天 10 万次请求的账算出来的（第三项「稳定会话 ID」是请求头模板 {{session_id}}，
-- 不需要建表：见 shared::channel::session_id_from_body）：
--
-- 1. `channel_models.drop_fields` —— 有的上游收下参数再回 400。opencode 的
--    `beam-free` 不认 `reasoning_effort`（Hermes 默认发 high），实测 28 次请求
--    整条失败。挂在「客户端模型名 → 上游模型名」这一行上：同一个上游对不同模型
--    认的参数不一样（同一条 ocgo 渠道上 `deepseek-v4.1-flash` 就认），挂渠道上
--    会把别的模型一起改坏。逗号分隔的顶层字段名。
ALTER TABLE channel_models ADD COLUMN drop_fields TEXT NOT NULL DEFAULT '';

-- 2. `channels.unwrap_envelope` —— cline.bot 的非流式响应被包了一层
--    `{"data": {<标准 OpenAI 响应>}, "success": true}`，不剥客户端读顶层
--    `choices` 只能读到空。默认 0 = 不剥（不认识的形状一个字节都不动）。
ALTER TABLE channels ADD COLUMN unwrap_envelope INTEGER NOT NULL DEFAULT 0;
"#,
    ),
];
