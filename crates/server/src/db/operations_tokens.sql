-- Independent, non-recoverable machine credentials and deployment identity.
CREATE TABLE operations_tokens (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 public_id TEXT NOT NULL UNIQUE,
 name TEXT NOT NULL,
 token_hash TEXT NOT NULL,
 created_by_account_id INTEGER NOT NULL REFERENCES panel_accounts(id),
 created_at_ms INTEGER NOT NULL,
 expires_at_ms INTEGER NOT NULL,
 revoked_at_ms INTEGER,
 last_used_at_ms INTEGER
);
CREATE INDEX operations_tokens_active ON operations_tokens(revoked_at_ms,expires_at_ms,id);
CREATE TABLE operations_instance (
 id INTEGER PRIMARY KEY CHECK(id=1),
 instance_id TEXT NOT NULL UNIQUE
);
