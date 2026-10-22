CREATE TABLE shared_virtual_models (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 normalized_name TEXT NOT NULL UNIQUE,
 enabled INTEGER NOT NULL,
 config_json TEXT NOT NULL,
 version INTEGER NOT NULL DEFAULT 1,
 actor_account_id INTEGER NOT NULL,
 created_ms INTEGER NOT NULL,
 updated_ms INTEGER NOT NULL
);
