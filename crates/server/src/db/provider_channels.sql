CREATE TABLE provider_channels (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL COLLATE NOCASE UNIQUE,
 enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
 created_ms INTEGER NOT NULL
);
CREATE TABLE provider_connections (
 channel_id INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
 provider_id INTEGER NOT NULL REFERENCES provider_channels(id) ON DELETE RESTRICT
);
CREATE INDEX provider_connections_parent ON provider_connections(provider_id);
ALTER TABLE channel_models ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1));
