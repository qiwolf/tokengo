ALTER TABLE provider_channels ADD COLUMN base_url TEXT;
ALTER TABLE provider_channels ADD COLUMN adapter_kind TEXT NOT NULL DEFAULT 'api';
ALTER TABLE provider_channels ADD COLUMN canonical_state TEXT NOT NULL DEFAULT 'attention';
ALTER TABLE provider_channels ADD COLUMN key_strategy TEXT NOT NULL DEFAULT 'ordered' CHECK(key_strategy IN ('ordered','concurrent'));
ALTER TABLE provider_channels ADD COLUMN switch_on_429 INTEGER NOT NULL DEFAULT 1 CHECK(switch_on_429 IN (0,1));
ALTER TABLE provider_channels ADD COLUMN switch_on_400 INTEGER NOT NULL DEFAULT 0 CHECK(switch_on_400 IN (0,1));
ALTER TABLE provider_channels ADD COLUMN config_version INTEGER NOT NULL DEFAULT 1;
CREATE TABLE provider_credentials (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 provider_id INTEGER NOT NULL REFERENCES provider_channels(id) ON DELETE RESTRICT,
 kind TEXT NOT NULL CHECK(kind IN ('api_key','oauth','anonymous')),
 label TEXT NOT NULL,
 enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
 priority INTEGER NOT NULL DEFAULT 0,
 max_concurrent INTEGER NOT NULL DEFAULT 0 CHECK(max_concurrent>=0),
 secret_source_id INTEGER REFERENCES channels(id) ON DELETE RESTRICT,
 oauth_account_id INTEGER REFERENCES oauth_accounts(id) ON DELETE RESTRICT,
 identity_hash TEXT NOT NULL,
 revision INTEGER NOT NULL DEFAULT 1,
 created_at_ms INTEGER NOT NULL
);
CREATE INDEX provider_credentials_parent ON provider_credentials(provider_id,priority,id);
CREATE TABLE source_credentials (
 source_id INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
 credential_id INTEGER NOT NULL REFERENCES provider_credentials(id) ON DELETE RESTRICT
);
CREATE INDEX source_credentials_owner ON source_credentials(credential_id,source_id);
CREATE TABLE credential_affinity (
 user_id INTEGER NOT NULL REFERENCES users(id),
 provider_id INTEGER NOT NULL REFERENCES provider_channels(id) ON DELETE CASCADE,
 credential_id INTEGER NOT NULL REFERENCES provider_credentials(id),
 generation INTEGER NOT NULL DEFAULT 1,
 updated_at_ms INTEGER NOT NULL,
 PRIMARY KEY(user_id,provider_id)
);
