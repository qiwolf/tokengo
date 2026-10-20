ALTER TABLE provider_channels ADD COLUMN endpoint_mode TEXT NOT NULL DEFAULT 'single' CHECK(endpoint_mode IN ('single','local'));
