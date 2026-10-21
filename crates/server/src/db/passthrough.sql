ALTER TABLE channels ADD COLUMN transport_kind TEXT NOT NULL DEFAULT 'llm' CHECK(transport_kind IN ('llm','passthrough'));
CREATE TABLE passthrough_routes (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 channel_id INTEGER NOT NULL UNIQUE REFERENCES channels(id),
 revision INTEGER NOT NULL DEFAULT 1,
 name TEXT NOT NULL,
 prefix TEXT NOT NULL UNIQUE,
 upstream TEXT NOT NULL,
 strip_prefix INTEGER NOT NULL,
 enabled INTEGER NOT NULL,
 price_id INTEGER NOT NULL REFERENCES billing_request_prices(id),
 timeout_secs INTEGER NOT NULL,
 max_concurrent INTEGER NOT NULL,
 max_body_bytes INTEGER NOT NULL,
 allowed_paths_json TEXT
);
CREATE TABLE passthrough_headers (
 route_id INTEGER NOT NULL REFERENCES passthrough_routes(id) ON DELETE CASCADE,
 name TEXT NOT NULL,
 nonce BLOB NOT NULL,
 cipher BLOB NOT NULL,
 PRIMARY KEY(route_id,name)
);
CREATE TABLE passthrough_acl (
 route_id INTEGER NOT NULL REFERENCES passthrough_routes(id) ON DELETE CASCADE,
 key_id INTEGER NOT NULL,
 PRIMARY KEY(route_id,key_id)
);
