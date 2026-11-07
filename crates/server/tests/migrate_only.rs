//! Offline migration must finish before background writers start.
use tokengo_server::db;
#[test]
fn migrate_only_preserves_business_rows_and_never_initializes_runtime_services() {
    let dir=std::env::temp_dir().join(format!("tokengo-migrate-only-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path=dir.join("db");
    {
        let c=rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT(datetime('now')))").unwrap();
        for (version,sql) in db::schema::MIGRATIONS.iter().filter(|(v,_)|*v<=44) {
            c.execute_batch(sql).unwrap();
            c.execute("INSERT INTO schema_migrations(version,applied_at) VALUES(?1,'preserved')",[version]).unwrap();
        }
        c.execute_batch("INSERT INTO requests(id,ts_ms,endpoint,model,status,error) VALUES(1,1,'chat','m','running','keep unchanged'); INSERT INTO model_invocations VALUES(1,'Model-A'),(1,'model_a'),(2,'Model-A'); INSERT INTO settings VALUES('migration-test-sentinel','unchanged');").unwrap();
    }
    for _ in 0..2 {
        let output=std::process::Command::new(env!("CARGO_BIN_EXE_tokengo-server"))
            .arg("--migrate-only").env_clear()
            .env("TOKENGO_DB",&path).env("TOKENGO_LISTEN","invalid-no-listener")
            .env("TOKENGO_MASTER_KEY_FILE",dir.join("must-not-create-key"))
            .env("TOKENGO_LOG_DIR",dir.join("must-not-create-log"))
            .output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let result:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["mode"],"migrate-only");assert_eq!(result["schema_version"],45);
    }
    let c=rusqlite::Connection::open(&path).unwrap();
    assert_eq!(c.query_row("SELECT status||':'||error FROM requests WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),"running:keep unchanged");
    assert_eq!(c.query_row("SELECT COUNT(*) FROM schema_migrations WHERE version<=44 AND applied_at='preserved'",[],|r|r.get::<_,i64>(0)).unwrap(),44);
    assert_eq!(c.query_row("SELECT value FROM settings WHERE key='migration-test-sentinel'",[],|r|r.get::<_,String>(0)).unwrap(),"unchanged");
    assert_eq!(c.query_row("SELECT total_calls FROM model_invocation_totals WHERE model_name='model a'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    assert!(!dir.join("must-not-create-key").exists());assert!(!dir.join("must-not-create-log").exists());
    drop(c);std::fs::remove_dir_all(dir).unwrap();
}
