//! Fsynced settlement intents, replayed before startup unknown-usage recovery.
//! Lock order is AppState DB mutex -> outbox file lock; no worker holds the file
//! lock while trying to acquire that mutex. SQLite settlement is itself idempotent.
use crate::{
    db::{self, DbError, Result},
    AppState, SharedState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Weak,
    time::Duration,
};
const VERSION: u32 = 1;
const DB_ID_KEY: &str = "billing_outbox_database_id";
const MAX_INTENT_BYTES: u64 = 16 * 1024;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settlement {
    version: u32,
    database_id: String,
    id: i64,
    attempt_id: String,
    input: Option<i64>,
    output: Option<i64>,
    status: String,
    attempts: u64,
    last_attempt_ms: Option<i64>,
}
fn bad(message: &str) -> DbError {
    DbError::Other(message.into())
}
fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|e| DbError::Other(format!("billing outbox I/O: {e}")))
}
fn outbox(db_path: &Path) -> Result<PathBuf> {
    let db_path = io(fs::canonicalize(db_path))?;
    let file = db_path
        .file_name()
        .ok_or_else(|| bad("invalid billing database filename"))?;
    let mut name = file.to_os_string();
    name.push(".billing-outbox");
    Ok(db_path.with_file_name(name))
}
fn sync_dir(path: &Path) -> Result<()> {
    io(io(File::open(path))?.sync_all())
}
fn prepare(dir: &Path) -> Result<()> {
    if !dir.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        io(builder.create(dir))?;
        if let Some(parent) = dir.parent() {
            sync_dir(parent)?;
        }
    }
    if io(fs::symlink_metadata(dir))?.file_type().is_symlink() {
        return Err(bad("billing outbox cannot be a symlink"));
    }
    Ok(())
}
fn lock(dir: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = io(options.open(dir.join(".lock")))?;
    io(file.lock())?;
    Ok(file)
}
fn database_id(conn: &Connection) -> Result<String> {
    if let Some(id) = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [DB_ID_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        uuid::Uuid::parse_str(&id).map_err(|_| bad("invalid billing database identity"))?;
        return Ok(id);
    }
    let candidate = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO settings(key,value) VALUES(?1,?2)",
        params![DB_ID_KEY, candidate],
    )?;
    let id: String = conn.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [DB_ID_KEY],
        |r| r.get(0),
    )?;
    uuid::Uuid::parse_str(&id).map_err(|_| bad("invalid billing database identity"))?;
    Ok(id)
}
fn atomic_write(dir: &Path, path: &Path, intent: &Settlement) -> Result<()> {
    let bytes =
        serde_json::to_vec(intent).map_err(|_| bad("cannot serialize billing settlement"))?;
    let temp = dir.join(format!(".{}.{}.tmp", intent.id, uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = io(options.open(&temp))?;
        io(file.write_all(&bytes))?;
        io(file.sync_all())?;
        io(fs::rename(&temp, path))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn load(path: &Path) -> Result<Settlement> {
    let meta = io(fs::symlink_metadata(path))?;
    if !meta.is_file() || meta.len() > MAX_INTENT_BYTES {
        return Err(bad("invalid billing intent file type or size"));
    }
    let bytes = io(fs::read(path))?;
    serde_json::from_slice(&bytes).map_err(|_| bad("invalid billing settlement JSON"))
}
fn validate(conn: &Connection, path: &Path, intent: &Settlement, database_id: &str) -> Result<()> {
    if intent.version != VERSION
        || intent.database_id != database_id
        || intent.id <= 0
        || intent.input.is_some_and(|n| n < 0)
        || intent.output.is_some_and(|n| n < 0)
        || intent.status == "reserved"
        || intent.status.is_empty()
        || intent.status.len() > 64
        || !intent
            .status
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(bad("billing intent identity or values do not match"));
    }
    if path.file_name().and_then(|v| v.to_str()) != Some(format!("{}.json", intent.id).as_str()) {
        return Err(bad("billing intent filename does not match reservation"));
    }
    let attempt: Option<String> = conn
        .query_row(
            "SELECT attempt_id FROM billing_ledger WHERE id=?1",
            [intent.id],
            |r| r.get(0),
        )
        .optional()?;
    if attempt.as_deref() != Some(intent.attempt_id.as_str()) {
        return Err(bad("billing intent reservation does not match database"));
    }
    Ok(())
}
fn remove(dir: &Path, path: &Path) -> Result<()> {
    io(fs::remove_file(path))?;
    sync_dir(dir)
}
fn quarantine(dir: &Path, path: &Path) -> Result<()> {
    let dead = dir.join("dead-letter");
    if !dead.exists() {
        io(fs::create_dir(&dead))?;
        sync_dir(dir)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| bad("invalid billing intent path"))?
        .to_string_lossy();
    let destination = dead.join(format!("{}.{}", name, uuid::Uuid::new_v4()));
    io(fs::rename(path, destination))?;
    sync_dir(&dead)?;
    sync_dir(dir)
}
fn deliver(conn: &Connection, dir: &Path, path: &Path, intent: &mut Settlement) -> Result<()> {
    conn.pragma_update(None, "synchronous", "FULL")?;
    intent.attempts = intent.attempts.saturating_add(1);
    intent.last_attempt_ms = Some(db::now_ms());
    atomic_write(dir, path, intent)?;
    match db::billing::settle(conn, intent.id, intent.input, intent.output, &intent.status) {
        Ok(()) => remove(dir, path),
        Err(error) => {
            tracing::warn!(
                reservation_id = intent.id,
                attempts = intent.attempts,
                "billing settlement remains pending: {error}"
            );
            Err(error)
        }
    }
}
/// The intent is fsynced before touching the billing ledger. A failed write leaves
/// the reservation held; callers must report that financial persistence failed.
pub fn settle(
    state: &SharedState,
    id: i64,
    input: Option<i64>,
    output: Option<i64>,
    outcome: &str,
) -> Result<()> {
    let conn = state.conn();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT attempt_id,status FROM billing_ledger WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((attempt_id, status)) = row else {
        return Err(bad("unknown billing reservation"));
    };
    if status != "reserved" {
        return Ok(());
    }
    let dir = outbox(&state.cfg.db_path)?;
    prepare(&dir)?;
    let _lock = lock(&dir)?;
    let database_id = database_id(&conn)?;
    let path = dir.join(format!("{id}.json"));
    let mut intent = Settlement {
        version: VERSION,
        database_id,
        id,
        attempt_id,
        input,
        output,
        status: outcome.into(),
        attempts: 0,
        last_attempt_ms: None,
    };
    validate(&conn, &path, &intent, &intent.database_id)?;
    if path.exists() {
        let existing = load(&path)?;
        validate(&conn, &path, &existing, &intent.database_id)?;
        let mut expected = existing.clone();
        expected.attempts = 0;
        expected.last_attempt_ms = None;
        if expected != intent {
            return Err(bad(
                "pending billing intent conflicts; known usage cannot be replaced",
            ));
        }
        intent = existing;
    } else {
        atomic_write(&dir, &path, &intent)?;
    }
    deliver(&conn, &dir, &path, &mut intent)
}
/// Replay all good intents, but fail on any failed/invalid intent or unresolved
/// dead letter. Startup must not fall through to unknown-usage recovery on error.
pub fn replay(conn: &Connection, db_path: &Path) -> Result<usize> {
    conn.pragma_update(None, "synchronous", "FULL")?;
    let dir = outbox(db_path)?;
    // Initialize the binding while startup DB writes are healthy. Normal sends
    // can then fsync an intent even if later SQLite writes are unavailable.
    let database_id = database_id(conn)?;
    if !dir.exists() {
        return Ok(0);
    }
    prepare(&dir)?;
    let _lock = lock(&dir)?;
    let mut paths = Vec::new();
    for entry in io(fs::read_dir(&dir))? {
        let path = io(entry)?.path();
        if path.extension().is_some_and(|x| x == "json" || x == "tmp") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut completed = 0;
    let mut failed = false;
    for path in paths {
        let checked = if path.extension().is_some_and(|x| x == "tmp") {
            Err(bad(
                "incomplete billing intent write requires reconciliation",
            ))
        } else {
            load(&path).and_then(|intent| {
                validate(conn, &path, &intent, &database_id)?;
                Ok(intent)
            })
        };
        match checked {
            Ok(mut intent) => match deliver(conn, &dir, &path, &mut intent) {
                Ok(()) => completed += 1,
                Err(_) => failed = true,
            },
            Err(error) => {
                tracing::error!(
                    "billing intent quarantined; manual reconciliation required: {error}"
                );
                quarantine(&dir, &path)?;
                failed = true;
            }
        }
    }
    let dead = dir.join("dead-letter");
    if dead.exists() && io(fs::read_dir(dead))?.next().is_some() {
        failed = true;
    }
    if failed {
        return Err(bad(
            "billing outbox pending or quarantined; recovery requires reconciliation",
        ));
    }
    Ok(completed)
}
/// Weak ownership avoids an Arc cycle; blocking filesystem work stays off Tokio's
/// async workers. Retry with 1..30 second backoff while retaining every intent.
pub fn spawn_retry(state: Weak<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seconds = 1;
        loop {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let weak = state.clone();
            let result = tokio::task::spawn_blocking(move || {
                let Some(state) = weak.upgrade() else {
                    return None;
                };
                let conn = state.conn();
                Some(replay(&conn, &state.cfg.db_path))
            })
            .await;
            match result {
                Ok(None) => break,
                Ok(Some(Ok(n))) => {
                    if n > 0 {
                        tracing::info!(settlements = n, "billing outbox replayed");
                    }
                    seconds = 1;
                }
                Ok(Some(Err(error))) => {
                    tracing::warn!(
                        retry_seconds = seconds,
                        "billing outbox retry pending: {error}"
                    );
                    seconds = (seconds * 2).min(30);
                }
                Err(error) => {
                    tracing::error!("billing outbox worker failed: {error}");
                    seconds = (seconds * 2).min(30);
                }
            }
        }
    })
}
