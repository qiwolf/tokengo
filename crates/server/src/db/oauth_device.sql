-- Device grants are short-lived encrypted transactions, never bearer-bearing DTOs.
CREATE TABLE oauth_device_pending (
 id TEXT PRIMARY KEY,
 provider TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('pending','complete','canceled','denied','expired')),
 expires_at INTEGER NOT NULL,
 next_poll_at INTEGER NOT NULL,
 interval_ms INTEGER NOT NULL,
 poll_owner TEXT,
 poll_lease_until INTEGER NOT NULL DEFAULT 0,
 nonce BLOB,
 cipher BLOB,
 account_id INTEGER REFERENCES oauth_accounts(id),
 CHECK((nonce IS NULL)=(cipher IS NULL))
);
CREATE INDEX oauth_device_expiry ON oauth_device_pending(expires_at);
