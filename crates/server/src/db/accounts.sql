-- Independent panel credentials; API user roles never grant panel root authority.
CREATE TABLE panel_accounts (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 username TEXT NOT NULL COLLATE NOCASE UNIQUE,
 display_name TEXT NOT NULL,
 role TEXT NOT NULL CHECK(role IN ('root','user')),
 user_id INTEGER REFERENCES users(id),
 enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
 password_hash TEXT NOT NULL,
 credential_version INTEGER NOT NULL DEFAULT 1,
 created_at_ms INTEGER NOT NULL,
 CHECK((role='root' AND user_id IS NULL AND enabled=1) OR (role='user' AND user_id IS NOT NULL))
);
CREATE UNIQUE INDEX panel_single_root ON panel_accounts(role) WHERE role='root';
INSERT INTO panel_accounts(username,display_name,role,password_hash,created_at_ms)
 SELECT 'admin','管理员','root',value,CAST(strftime('%s','now') AS INTEGER)*1000
 FROM settings WHERE key='panel_password_hash';
CREATE TABLE account_sessions (
 session_digest TEXT PRIMARY KEY NOT NULL,
 account_id INTEGER NOT NULL REFERENCES panel_accounts(id) ON DELETE CASCADE,
 credential_version INTEGER NOT NULL,
 created_at_ms INTEGER NOT NULL,
 expires_at_ms INTEGER NOT NULL
);
CREATE INDEX account_sessions_owner ON account_sessions(account_id,created_at_ms);
CREATE TRIGGER panel_credentials_revoke AFTER UPDATE OF password_hash,username,enabled ON panel_accounts
 WHEN OLD.password_hash<>NEW.password_hash OR OLD.username<>NEW.username OR OLD.enabled<>NEW.enabled
 BEGIN
  UPDATE panel_accounts SET credential_version=OLD.credential_version+1 WHERE id=NEW.id;
  DELETE FROM account_sessions WHERE account_id=NEW.id;
  DELETE FROM panel_sessions WHERE NEW.role='root';
 END;
CREATE TRIGGER panel_linked_user_disabled AFTER UPDATE OF enabled ON users
 WHEN OLD.enabled<>0 AND NEW.enabled=0
 BEGIN
  DELETE FROM account_sessions WHERE account_id IN (SELECT id FROM panel_accounts WHERE user_id=NEW.id);
  UPDATE panel_accounts SET credential_version=credential_version+1 WHERE user_id=NEW.id;
 END;
