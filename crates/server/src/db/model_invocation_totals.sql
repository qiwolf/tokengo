-- Derived, rebuildable state. Canonicalization remains in Rust so spellings,
-- provider suffixes and per-request retry deduplication keep the existing rules.
CREATE TABLE model_invocation_dirty(request_id INTEGER PRIMARY KEY);
CREATE TABLE model_invocation_canonical(
 request_id INTEGER NOT NULL, model_name TEXT NOT NULL,
 PRIMARY KEY(request_id,model_name));
CREATE TABLE model_invocation_totals(
 model_name TEXT PRIMARY KEY NOT NULL, total_calls INTEGER NOT NULL CHECK(total_calls>=0));
CREATE TRIGGER model_invocation_dirty_insert AFTER INSERT ON model_invocations BEGIN
 INSERT OR IGNORE INTO model_invocation_dirty VALUES(NEW.request_id);
END;
CREATE TRIGGER model_invocation_dirty_delete AFTER DELETE ON model_invocations BEGIN
 INSERT OR IGNORE INTO model_invocation_dirty VALUES(OLD.request_id);
END;
CREATE TRIGGER model_invocation_dirty_update AFTER UPDATE ON model_invocations BEGIN
 INSERT OR IGNORE INTO model_invocation_dirty VALUES(OLD.request_id);
 INSERT OR IGNORE INTO model_invocation_dirty VALUES(NEW.request_id);
END;
INSERT INTO model_invocation_dirty SELECT DISTINCT request_id FROM model_invocations;
