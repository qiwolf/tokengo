-- Additive identity migration. Existing credentials belong to the legacy administrator.
CREATE TABLE users (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('admin','user')),
 enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
 policy_json TEXT NOT NULL DEFAULT '{}',
 created_at_ms INTEGER NOT NULL
);
INSERT INTO users(id,name,role,created_at_ms) VALUES(1,'Legacy administrator','admin',CAST(strftime('%s','now') AS INTEGER)*1000);
-- SQLite forbids ADD COLUMN REFERENCES with a non-NULL default on populated tables.
-- Validate ownership with triggers instead, preserving every historical key row and ID.
ALTER TABLE gateway_keys ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
CREATE TRIGGER gateway_key_owner_insert BEFORE INSERT ON gateway_keys
WHEN NOT EXISTS(SELECT 1 FROM users WHERE id=NEW.user_id) BEGIN
 SELECT RAISE(ABORT,'Unknown key owner');
END;
CREATE TRIGGER gateway_key_owner_update BEFORE UPDATE OF user_id ON gateway_keys
WHEN NOT EXISTS(SELECT 1 FROM users WHERE id=NEW.user_id) BEGIN
 SELECT RAISE(ABORT,'Unknown key owner');
END;
ALTER TABLE gateway_keys ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE gateway_keys ADD COLUMN revoked_at INTEGER;
ALTER TABLE gateway_keys ADD COLUMN expires_at INTEGER;
ALTER TABLE gateway_keys ADD COLUMN policy_json TEXT NOT NULL DEFAULT '{}';
-- Keep parent IDs even if the legacy compatibility endpoint physically removes a key.
-- A missing ancestor then invalidates its children; ledger attribution remains intact.
ALTER TABLE gateway_keys ADD COLUMN parent_key_id INTEGER;
CREATE INDEX gateway_keys_user ON gateway_keys(user_id,id);
CREATE TABLE audit_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 ts_ms INTEGER NOT NULL,
 actor_user_id INTEGER,
 actor_key_id INTEGER,
 action TEXT NOT NULL,
 target_type TEXT NOT NULL,
 target_id INTEGER,
 metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_events_ts ON audit_events(ts_ms DESC,id DESC);
