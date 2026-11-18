CREATE TABLE model_specifications (
    model_id TEXT NOT NULL,
    provider_id INTEGER REFERENCES provider_channels(id) ON DELETE CASCADE,
    source_url TEXT,
    specification TEXT NOT NULL
);
CREATE UNIQUE INDEX model_specifications_provider
    ON model_specifications(model_id, provider_id) WHERE provider_id IS NOT NULL;
CREATE UNIQUE INDEX model_specifications_global
    ON model_specifications(model_id) WHERE provider_id IS NULL;

CREATE TABLE model_specification_aliases (
    alias_id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL CHECK(model_id <> alias_id)
);

CREATE TABLE model_specification_relay_defaults (
    relay TEXT NOT NULL,
    model_id TEXT NOT NULL,
    source_url TEXT NOT NULL,
    specification TEXT NOT NULL,
    PRIMARY KEY(relay, model_id)
);
