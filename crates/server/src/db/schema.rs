//! 库表定义。迁移按版本号顺序执行，已执行的记在 `schema_migrations` 里。
//!
//! 设计取舍（对应 `docs/02-按需功能清单.md`）：
//! - 现网 FreeLLMAPI 33 张表，本机真在用的只有几张；这里按实际需要重建，共 11 张。
//! - **免费模型目录相关的表一张都没有**（models / embedding_models / quirks /
//!   catalog_* / media_models …）——用户明确不要，且那正是内存与库体积持续增长的来源。
//! - `leg_headers` 是上游没有的增项：按腿配自定义请求头。

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
];
