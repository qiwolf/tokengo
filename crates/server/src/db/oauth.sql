-- OAuth credentials belong to registrations, independently of source deployment bindings.
CREATE TABLE oauth_hosts (id INTEGER PRIMARY KEY CHECK(id=1),host_id TEXT NOT NULL);
CREATE TABLE oauth_accounts (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 label TEXT NOT NULL,
 issuer TEXT NOT NULL,
 subject TEXT NOT NULL,
 client_id TEXT NOT NULL,
 email TEXT,
 status TEXT NOT NULL CHECK(status IN ('active','revoked','reauth_required')),
 scopes_json TEXT NOT NULL,
 expires_at INTEGER NOT NULL,
 earliest_refresh_at INTEGER NOT NULL,
 credential_revision INTEGER NOT NULL DEFAULT 1,
 nonce BLOB,
 cipher BLOB,
 refresh_owner TEXT,
 refresh_lease_until INTEGER NOT NULL DEFAULT 0,
 UNIQUE(issuer,client_id,subject)
);
CREATE TABLE oauth_pending (
 id TEXT PRIMARY KEY,
 state_digest TEXT NOT NULL UNIQUE,
 expires_at INTEGER NOT NULL,
 consumed INTEGER NOT NULL DEFAULT 0 CHECK(consumed IN (0,1)),
 nonce BLOB NOT NULL,
 cipher BLOB NOT NULL
);
CREATE TABLE oauth_channel_bindings (
 channel_id INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
 account_id INTEGER NOT NULL REFERENCES oauth_accounts(id),
 reasoning_effort TEXT CHECK(reasoning_effort IN ('low','medium','high')),
 max_input_tokens INTEGER CHECK(max_input_tokens BETWEEN 1 AND 16777216),
 max_output_tokens INTEGER CHECK(max_output_tokens BETWEEN 1 AND 16777216),
 service_tier TEXT CHECK(service_tier IN ('priority','auto','default'))
);
CREATE TRIGGER oauth_binding_insert AFTER INSERT ON oauth_channel_bindings BEGIN
 UPDATE channels SET binding_generation=binding_generation+1 WHERE id=NEW.channel_id;
END;
CREATE TRIGGER oauth_binding_update AFTER UPDATE ON oauth_channel_bindings
WHEN OLD.account_id<>NEW.account_id OR OLD.reasoning_effort IS NOT NEW.reasoning_effort OR OLD.service_tier IS NOT NEW.service_tier OR OLD.max_input_tokens IS NOT NEW.max_input_tokens OR OLD.max_output_tokens IS NOT NEW.max_output_tokens BEGIN
 UPDATE channels SET binding_generation=binding_generation+1 WHERE id=NEW.channel_id;
END;
CREATE TRIGGER oauth_binding_delete AFTER DELETE ON oauth_channel_bindings BEGIN
 UPDATE channels SET binding_generation=binding_generation+1 WHERE id=OLD.channel_id;
END;
CREATE TRIGGER oauth_account_revoke AFTER UPDATE OF status ON oauth_accounts
WHEN NEW.status='revoked' AND OLD.status<>'revoked' BEGIN
 UPDATE channels SET binding_generation=binding_generation+1 WHERE id IN (SELECT channel_id FROM oauth_channel_bindings WHERE account_id=NEW.id);
END;
