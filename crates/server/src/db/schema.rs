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
    (
        5,
        r#"
-- 模型组的「类型」：对话 / 向量 / 重排（2026-10-17，v0.3.3）。
--
-- 为什么要有它：同一个网关后面挂着三种用途完全不同的模型 —— 向量（现网 86% 的量）、
-- 对话、重排。面板上不分类型就得靠人记名字；探活也必须知道该打哪个端点
-- （拿对话请求去探向量组必然 400）。所以类型是**模型组自己的属性**，写进库。
--
-- 怎么定下来的：默认空 = 「还没定」，第一次真收到请求时按走的端点自动记上
-- （kind_source = 'auto'）；人在面板上选过一次之后 kind_source = 'manual'，
-- **再也不被实测覆盖** —— 用户拍过的板不许被流量改掉。
ALTER TABLE model_groups ADD COLUMN kind TEXT NOT NULL DEFAULT '';
ALTER TABLE model_groups ADD COLUMN kind_source TEXT NOT NULL DEFAULT '';
"#,
    ),
    (
        6,
        r#"
-- 这条请求是不是在让模型看图（2026-10-18，v0.3.10）。
--
-- 为什么要有它：用户报「加了图片，模型识别不了」时，**分不清是客户端根本没把图发上来，
-- 还是网关把图丢了，还是上游看不见**。记下这一位，面板「请求」页就能直接回答这个问题。
-- 判定口径与路由用的完全一致（tokengo_shared::request_has_image），不另立一套。
ALTER TABLE requests ADD COLUMN has_image INTEGER NOT NULL DEFAULT 0;
"#,
    ),
    (
        7,
        r#"
-- 每条渠道「同时最多几发」（2026-10-18，v0.5.0）。
--
-- 上游的免费额度普遍是按**凭据**限并发的：同一把 key 上同时压 50 发，上游直接回 429/503。
-- 现网 4 天的账里 503「bridge busy」151 次、502「上游不可用」168 次，就是这一类。
-- 0 = 不限（默认，行为与以前完全一致）；配了就在网关门口排队/改走别的渠道。
-- 挂渠道而不是挂模型组：并发是上游那把 key 的属性，同一个模型组下每条渠道的上限额各不相同。
ALTER TABLE channels ADD COLUMN max_concurrent INTEGER NOT NULL DEFAULT 0;
"#,
    ),
    (
        8,
        r#"
-- 渠道的「上游目录」缓存 + 一问时间（2026-10-18，v0.7.0）。
--
-- 以前「问一下上游有哪些模型」纯粹是个屏幕动作：拉到就画出来，关掉弹窗就没了，
-- 下次还得再问一遍（有的上游要花钱、有限流）。更要紧的是「这条渠道真能提供哪些名字」
-- 在库里没有持久事实 —— 只能靠人工在渠道里勾，时间一长没人说得清上游到底报过什么。
--
-- 这里把上游每次报回来的清单**原样记住**：上游没说的字段一律存 NULL（绝不替它猜），
-- 面板打开弹窗先读它、写清「上次问到几点」，点按钮才真去问一次。
-- 失败时不写：一次连不上不能把上次真拿到手的清单删掉。
--
-- catalog_at 挂在渠道行上：一条渠道一份清单，一起写一起读，面板拿渠道列表就能显示时间。
CREATE TABLE channel_catalog (
    channel_id     INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    model_id       TEXT    NOT NULL,
    owned_by       TEXT,
    context_window INTEGER,
    price_note     TEXT,
    is_free        INTEGER,
    vision_claim   INTEGER,
    PRIMARY KEY (channel_id, model_id)
);

ALTER TABLE channels ADD COLUMN catalog_at INTEGER NOT NULL DEFAULT 0;
"#,
    ),
    (
        9,
        r#"
-- 「模型名 → 用途」独立成表（2026-10-18，v0.10.0）。
--
-- 原来用途只挂在模型组上（迁移 v5）。面板「模型」页改成以「渠道里勾过的名字」为行
-- 之后，没建过同名模型的名字就没地方标用途，图像 / 视频 / 音频 这类网关暂不转发、只能手标的
-- 名字永远归不了档。所以用途改成挂在**名字**上；那一层（v0.11.0 起已拿掉）那份继续写，
-- 由 db::groups 的两个写入点成对维护。
--
-- 这一步只做两件事：建表 + 把那一层上已有的非空类型搬过来。幂等：重复跑不改数据。
-- 不碰任何现有表 —— 回滚就是丢掉这张表，路由数据一个字节不动。
CREATE TABLE IF NOT EXISTS model_kinds (
    model_name  TEXT PRIMARY KEY,
    kind        TEXT NOT NULL DEFAULT '',
    kind_source TEXT NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO model_kinds (model_name, kind, kind_source, updated_at)
SELECT name, kind, kind_source, 0 FROM model_groups WHERE kind <> '';
"#,
    ),
    (
        10,
        r#"
-- 把「模型组」这一层拿掉（2026-10-18，v0.11.0。用户：「把模型组这个模式彻底移除」）。
--
-- 以前：模型名 → 模型组（有序渠道列表 + 启用开关），渠道里的勾选只是白名单。
-- 现在：模型名 → 直接就是「勾了它的渠道」，按**渠道自己的全局顺序**试。
--
-- 顺序落在 channels.priority 上：小的先试，相同按 id。回填规则 = 取这条渠道在
-- **成员最多的那个组**里的位次，没进过任何组的排到 1000+id（排在最后）。
-- 现网只读算过一遍（2026-10-18，v9 库）：auto 组 5 条成员按原顺序落到 0..4
-- （opencode-go → opencode-go 2 → cline.bot → command-go → gateway.example.com），
-- 硅基流动只在 `BAAI/bge-m3` 那个单人组里 → 位次 0。**先后按 (priority, id) 排**，
-- 于是硅基流动排在 opencode-go 前面（它不服务 auto，这个次序今天谁都不受影响：
-- 迁移后每个真实名字下都只有一条渠道）。
--
-- 另一列是给「请求」账用的：auto 挑中了谁，面板上要看得见。客户端打的 model 还是
-- auto（那是请求级的事实），served_model 记的是真正被叫的那个模型名（路由级的事实）。
--
-- **model_groups / model_group_channels 一张都不删**：不用的东西留在原地，
-- 回滚（换回上一个二进制）时它们还是完整的。只加两列，一行现有数据都不动。
ALTER TABLE channels ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
UPDATE channels SET priority = COALESCE(
    (SELECT m.ordinal FROM model_group_channels m
      WHERE m.channel_id = channels.id
      ORDER BY (SELECT COUNT(*) FROM model_group_channels x WHERE x.group_name = m.group_name) DESC,
               m.group_name
      LIMIT 1),
    1000 + channels.id);
ALTER TABLE requests ADD COLUMN served_model TEXT NOT NULL DEFAULT '';
"#,
    ),
    (11, r#"ALTER TABLE channel_catalog ADD COLUMN kind_claim TEXT;"#),
    (12, r#"CREATE TABLE model_type_checks (
      channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
      model_name TEXT NOT NULL, fingerprint TEXT NOT NULL,
      tries INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL DEFAULT 0,
      status TEXT NOT NULL DEFAULT '', PRIMARY KEY(channel_id,model_name));"#),
    (13,r#"ALTER TABLE requests ADD COLUMN user_agent TEXT;
CREATE TABLE model_vision_checks (
      channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
      model_name TEXT NOT NULL, fingerprint TEXT NOT NULL, tries INTEGER NOT NULL DEFAULT 0,
      next_at INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT '',
      PRIMARY KEY(channel_id,model_name));"#),
    (14,r#"ALTER TABLE attempts ADD COLUMN model_name TEXT;
ALTER TABLE attempts ADD COLUMN upstream_model TEXT;
ALTER TABLE attempts ADD COLUMN first_byte_ms INTEGER;
ALTER TABLE requests ADD COLUMN error_code TEXT;
CREATE INDEX idx_attempts_model ON attempts(model_name,channel_id,request_id);"#),
    (15,r#"CREATE TABLE model_cooldowns (
channel_id INTEGER NOT NULL,model_name TEXT NOT NULL,upstream_model TEXT NOT NULL,
failures INTEGER NOT NULL DEFAULT 0,until_ms INTEGER NOT NULL DEFAULT 0,
reason TEXT NOT NULL DEFAULT '',generation INTEGER NOT NULL DEFAULT 0,lease_until INTEGER NOT NULL DEFAULT 0,
PRIMARY KEY(channel_id,model_name,upstream_model));
ALTER TABLE requests ADD COLUMN explored INTEGER NOT NULL DEFAULT 0;"#),
    (16,r#"
-- Independent of request retention: preserve one attributable invocation per request/model.
CREATE TABLE model_invocations(request_id INTEGER NOT NULL,model_name TEXT NOT NULL,PRIMARY KEY(request_id,model_name));
INSERT OR IGNORE INTO model_invocations SELECT request_id,model_name FROM attempts WHERE TRIM(COALESCE(model_name,''))<>'' AND LOWER(TRIM(model_name))<>'auto';
INSERT OR IGNORE INTO model_invocations SELECT id,CASE WHEN TRIM(served_model)<>'' THEN served_model ELSE model END FROM requests WHERE channel_id IS NOT NULL AND LOWER(TRIM(CASE WHEN TRIM(served_model)<>'' THEN served_model ELSE model END))<>'auto';
CREATE TRIGGER model_invocation_insert AFTER INSERT ON attempts WHEN TRIM(COALESCE(NEW.model_name,''))<>'' AND LOWER(TRIM(NEW.model_name))<>'auto' BEGIN
 INSERT OR IGNORE INTO model_invocations VALUES(NEW.request_id,NEW.model_name);
END;
"#),
    (17, r#"CREATE TABLE registry_context (
        model_id TEXT PRIMARY KEY, context_window INTEGER, reason TEXT NOT NULL);
    CREATE TABLE registry_context_status (
        id INTEGER PRIMARY KEY CHECK(id=1), fetched_at INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER NOT NULL DEFAULT 0, tries INTEGER NOT NULL DEFAULT 0,
        origin TEXT NOT NULL DEFAULT '', error TEXT NOT NULL DEFAULT '');
    INSERT INTO registry_context_status(id) VALUES(1);"#),
    (18, r#"CREATE TABLE registry_associations (
        channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
        model_name TEXT NOT NULL, upstream_model TEXT NOT NULL, model_id TEXT NOT NULL,
        PRIMARY KEY(channel_id, model_name, upstream_model));
    CREATE TABLE channel_context_overrides (
        channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
        upstream_model TEXT NOT NULL, context_window INTEGER NOT NULL CHECK(context_window>0),
        PRIMARY KEY(channel_id,upstream_model));"#),
    (19,r#"
DELETE FROM channel_models WHERE LOWER(TRIM(model_name))='auto';
DELETE FROM registry_associations WHERE LOWER(TRIM(model_name))='auto';
DELETE FROM model_type_checks WHERE LOWER(TRIM(model_name))='auto';
DELETE FROM model_vision_checks WHERE LOWER(TRIM(model_name))='auto';
"#),
    (20,r#"
DELETE FROM registry_associations;
DELETE FROM model_type_checks WHERE EXISTS (SELECT 1 FROM channel_models m WHERE m.channel_id=model_type_checks.channel_id AND m.model_name=model_type_checks.model_name AND TRIM(COALESCE(m.upstream_model,''))<>'' AND TRIM(m.upstream_model)<>TRIM(m.model_name));
DELETE FROM model_vision_checks WHERE EXISTS (SELECT 1 FROM channel_models m WHERE m.channel_id=model_vision_checks.channel_id AND m.model_name=model_vision_checks.model_name AND TRIM(COALESCE(m.upstream_model,''))<>'' AND TRIM(m.upstream_model)<>TRIM(m.model_name));
DELETE FROM channel_models WHERE TRIM(COALESCE(upstream_model,''))<>'' AND TRIM(upstream_model)<>TRIM(model_name);
UPDATE channel_models SET upstream_model=NULL;
"#),
    (21, r#"
CREATE TABLE gateway_keys (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL, hint TEXT NOT NULL, created_at_ms INTEGER NOT NULL,
 key_hash TEXT NOT NULL, nonce BLOB, cipher BLOB,
 selector TEXT UNIQUE, is_default INTEGER NOT NULL DEFAULT 0,
 legacy_sealed TEXT);
CREATE UNIQUE INDEX gateway_keys_default ON gateway_keys(is_default) WHERE is_default=1;
INSERT INTO gateway_keys(name,hint,created_at_ms,key_hash,is_default,legacy_sealed)
 SELECT 'Legacy', 'Legacy key (hash only; cannot reveal)', CAST(strftime('%s','now') AS INTEGER)*1000,
 value, 1, (SELECT value FROM settings WHERE key='gateway_key_sealed')
 FROM settings WHERE key='gateway_key_hash' AND value<>'';
DELETE FROM settings WHERE key IN ('gateway_key_hash','gateway_key_sealed');
"#),
    (22, "ALTER TABLE channel_headers ADD COLUMN sealed INTEGER NOT NULL DEFAULT 0;"),
    (23, "ALTER TABLE requests ADD COLUMN input_tokens_known INTEGER NOT NULL DEFAULT 0;
ALTER TABLE requests ADD COLUMN output_tokens_known INTEGER NOT NULL DEFAULT 0;
ALTER TABLE requests ADD COLUMN usage_source TEXT NOT NULL DEFAULT 'legacy';
UPDATE requests SET input_tokens_known=(input_tokens>0), output_tokens_known=(output_tokens>0);"),
    (24, r#"
ALTER TABLE channels ADD COLUMN fault_revision INTEGER NOT NULL DEFAULT 0;
CREATE TABLE channel_faults (
 channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
 model TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL, reason TEXT NOT NULL,
 occurred_ms INTEGER NOT NULL, revision INTEGER NOT NULL,
 retry_at INTEGER NOT NULL DEFAULT 0, ready INTEGER NOT NULL DEFAULT 0,
 lease_until INTEGER NOT NULL DEFAULT 0, generation INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(channel_id,model));
CREATE TRIGGER fault_config_url AFTER UPDATE OF base_url ON channels WHEN OLD.base_url<>NEW.base_url BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=NEW.id;
 UPDATE channel_faults SET revision=NEW.fault_revision+1,ready=1,lease_until=0,generation=generation+1 WHERE channel_id=NEW.id;
END;
CREATE TRIGGER fault_secret_insert AFTER INSERT ON channel_secrets BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=NEW.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=NEW.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=NEW.channel_id;
END;
CREATE TRIGGER fault_secret_update AFTER UPDATE ON channel_secrets BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=NEW.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=NEW.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=NEW.channel_id;
END;
CREATE TRIGGER fault_secret_delete AFTER DELETE ON channel_secrets BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=OLD.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=OLD.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=OLD.channel_id;
END;
CREATE TRIGGER fault_header_insert AFTER INSERT ON channel_headers BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=NEW.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=NEW.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=NEW.channel_id;
END;
CREATE TRIGGER fault_header_update AFTER UPDATE ON channel_headers BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=NEW.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=NEW.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=NEW.channel_id;
END;
CREATE TRIGGER fault_header_delete AFTER DELETE ON channel_headers BEGIN
 UPDATE channels SET fault_revision=fault_revision+1 WHERE id=OLD.channel_id;
 UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=OLD.channel_id),ready=1,lease_until=0,generation=generation+1 WHERE channel_id=OLD.channel_id;
END;
"#),
    (25, include_str!("identity.sql")),
    (26, include_str!("billing.sql")),
    (27, include_str!("../admission.sql")),
    (28, include_str!("gateway_identity.sql")),
    (29, concat!(include_str!("protocols.sql"), "\n", include_str!("protocol_state.sql"))),
    (30, include_str!("oauth.sql")),
    (31, include_str!("search_billing.sql")),
];
