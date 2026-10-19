-- Rolling 60-second request/token reservations. No foreign keys: revocation must
-- not erase consumption still relevant to user, key, or parent-key windows.
CREATE TABLE quota_events (
    scope TEXT NOT NULL CHECK (scope IN ('user', 'key')),
    scope_id INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    tokens INTEGER NOT NULL CHECK (tokens >= 0)
);
CREATE INDEX quota_events_scope_window ON quota_events(scope, scope_id, at_ms);
CREATE INDEX quota_events_expiration ON quota_events(at_ms);
