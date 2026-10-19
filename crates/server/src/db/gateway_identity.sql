ALTER TABLE requests ADD COLUMN user_id INTEGER;
ALTER TABLE requests ADD COLUMN key_id INTEGER;
ALTER TABLE requests ADD COLUMN auth_snapshot TEXT;
ALTER TABLE attempts ADD COLUMN user_id INTEGER;
ALTER TABLE attempts ADD COLUMN key_id INTEGER;
ALTER TABLE attempts ADD COLUMN auth_snapshot TEXT;
ALTER TABLE billing_ledger ADD COLUMN auth_snapshot TEXT NOT NULL DEFAULT '{}';
ALTER TABLE billing_ledger ADD COLUMN source_snapshot TEXT NOT NULL DEFAULT '{}';
CREATE TRIGGER attempt_identity AFTER INSERT ON attempts BEGIN
 UPDATE attempts SET
 user_id=(SELECT user_id FROM requests WHERE id=NEW.request_id),
 key_id=(SELECT key_id FROM requests WHERE id=NEW.request_id),
 auth_snapshot=COALESCE((SELECT auth_snapshot FROM billing_ledger WHERE attempt_id=CAST(NEW.request_id AS TEXT)||':'||CAST(NEW.ordinal AS TEXT)),(SELECT auth_snapshot FROM requests WHERE id=NEW.request_id))
 WHERE id=NEW.id;
END;
