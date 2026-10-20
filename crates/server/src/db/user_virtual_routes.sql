-- Private routing metadata only; existing user/key/source/ledger identities remain unchanged.
CREATE TABLE user_virtual_models (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
 name TEXT NOT NULL, normalized_name TEXT NOT NULL,
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
 mode TEXT NOT NULL CHECK(mode IN ('ordered','scheduled')),
 timezone TEXT NOT NULL CHECK(timezone IN ('Asia/Shanghai','UTC')),
 default_positions_json TEXT NOT NULL,
 version INTEGER NOT NULL CHECK(version>0), created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
 UNIQUE(user_id,normalized_name)
);
CREATE TABLE user_virtual_targets (
 virtual_model_id INTEGER NOT NULL REFERENCES user_virtual_models(id) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 16),
 provider_id INTEGER NOT NULL REFERENCES provider_channels(id) ON DELETE RESTRICT,
 logical_model TEXT NOT NULL,
 switch_on_429 INTEGER NOT NULL CHECK(switch_on_429 IN (0,1)),
 switch_on_400 INTEGER NOT NULL CHECK(switch_on_400 IN (0,1)),
 PRIMARY KEY(virtual_model_id,position)
);
CREATE TABLE user_virtual_windows (
 virtual_model_id INTEGER NOT NULL REFERENCES user_virtual_models(id) ON DELETE CASCADE,
 position INTEGER NOT NULL,
 days_mask INTEGER NOT NULL CHECK(days_mask BETWEEN 1 AND 127),
 start_minute INTEGER NOT NULL CHECK(start_minute BETWEEN 0 AND 1439),
 end_minute INTEGER NOT NULL CHECK(end_minute BETWEEN 1 AND 1440 AND end_minute>start_minute),
 target_positions_json TEXT NOT NULL,
 PRIMARY KEY(virtual_model_id,position)
);
