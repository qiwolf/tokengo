-- Only allowlisted capability metadata, never upstream raw catalog or credentials.
CREATE TABLE channel_reasoning_catalog (
 channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
 upstream_model TEXT NOT NULL,
 source_identity TEXT NOT NULL,
 capability_json TEXT NOT NULL,
 fetched_at_ms INTEGER NOT NULL,
 PRIMARY KEY(channel_id,upstream_model)
);
CREATE TABLE channel_reasoning_settings (
 channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
 model_name TEXT NOT NULL,
 upstream_model TEXT NOT NULL,
 source_identity TEXT NOT NULL,
 manual_levels TEXT,
 default_effort TEXT,
 updated_at_ms INTEGER NOT NULL,
 PRIMARY KEY(channel_id,model_name)
);
