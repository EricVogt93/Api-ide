//! SQLite-backed execution history store.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::exec::ExecutionResult;

/// Bodies larger than this are truncated before being stored, with
/// `truncated` set to `true` on the row.
pub const MAX_STORED_BODY_BYTES: usize = 512 * 1024;

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("history database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("history serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type HistoryResult<T> = Result<T, HistoryError>;

/// A single stored history row with full request/response bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    /// RFC3339 timestamp.
    pub executed_at: String,
    pub request_id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: i64,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<Vec<u8>>,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Option<Vec<u8>>,
    pub error: Option<String>,
    pub env: Option<String>,
    /// Assertion verdict of the run, when the adapter knows one:
    /// `Some(true)` all assertions passed, `Some(false)` at least one
    /// failed, `None` no verdict (ad-hoc send or transport error).
    pub passed: Option<bool>,
    /// `true` if either body was truncated to fit `MAX_STORED_BODY_BYTES`.
    pub truncated: bool,
}

/// Lightweight row for list views — no bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySummary {
    pub id: i64,
    pub executed_at: String,
    pub request_id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: i64,
    pub error: Option<String>,
    /// See [`HistoryEntry::passed`].
    pub passed: Option<bool>,
}

/// Filter/pagination options for [`HistoryStore::list`].
#[derive(Debug, Clone)]
pub struct HistoryFilter {
    /// Case-insensitive substring match against `name` or `url`.
    pub text: Option<String>,
    pub method: Option<String>,
    pub status_min: Option<u16>,
    pub status_max: Option<u16>,
    pub request_id: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        Self {
            text: None,
            method: None,
            status_min: None,
            status_max: None,
            request_id: None,
            limit: 200,
            offset: 0,
        }
    }
}

/// Input to [`HistoryStore::record`]. Status/duration/response
/// headers/body/error are all derived from `outcome`.
pub struct NewEntry<'a> {
    pub request_id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub env: Option<String>,
    /// `Ok` for a completed HTTP exchange, `Err` for a transport-level
    /// failure message (connect, timeout, cancelled, ...).
    pub outcome: Result<&'a ExecutionResult, &'a str>,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<Vec<u8>>,
}

/// Fully-owned input for adapters whose execution result is not the legacy
/// [`ExecutionResult`] type.
pub struct HistoryRecord {
    pub executed_at: String,
    pub request_id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: i64,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<Vec<u8>>,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Option<Vec<u8>>,
    pub error: Option<String>,
    pub env: Option<String>,
    /// See [`HistoryEntry::passed`].
    pub passed: Option<bool>,
}

/// SQLite-backed execution history.
pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    /// Open (creating if needed) a history database at `path`, in WAL mode.
    pub fn open(path: &Path) -> HistoryResult<Self> {
        let conn = Connection::open(path)?;
        let _: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open a private in-memory database — used by tests and as a
    /// GUI fallback when no workspace path is available.
    pub fn open_in_memory() -> HistoryResult<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Record one executed request, returning its new row id.
    pub fn record(&self, entry: NewEntry<'_>) -> HistoryResult<i64> {
        let (executed_at, status, duration_ms, response_headers, response_body, error) =
            match entry.outcome {
                Ok(exec) => (
                    exec.executed_at.to_rfc3339(),
                    Some(exec.status),
                    exec.timing.total.as_millis() as i64,
                    exec.headers.clone(),
                    Some(exec.body.clone()),
                    None,
                ),
                Err(message) => (
                    Utc::now().to_rfc3339(),
                    None,
                    0i64,
                    Vec::new(),
                    None,
                    Some(message.to_string()),
                ),
            };

        self.record_raw(HistoryRecord {
            executed_at,
            request_id: entry.request_id,
            name: entry.name,
            method: entry.method,
            url: entry.url,
            status,
            duration_ms,
            request_headers: entry.request_headers,
            request_body: entry.request_body,
            response_headers,
            response_body,
            error,
            env: entry.env,
            passed: None,
        })
    }

    /// Record a fully-owned execution produced by another request adapter.
    pub fn record_raw(&self, entry: HistoryRecord) -> HistoryResult<i64> {
        let (request_body, request_truncated) = cap_body(entry.request_body);
        let (response_body, response_truncated) = cap_body(entry.response_body);
        let truncated = request_truncated || response_truncated;

        let request_headers_json = serde_json::to_string(&entry.request_headers)?;
        let response_headers_json = serde_json::to_string(&entry.response_headers)?;

        let conn = self.conn();
        conn.execute(
            "INSERT INTO entries (
                executed_at, request_id, name, method, url, status, duration_ms,
                request_headers, request_body, response_headers, response_body,
                error, env, truncated, passed
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                entry.executed_at,
                entry.request_id,
                entry.name,
                entry.method,
                entry.url,
                entry.status,
                entry.duration_ms,
                request_headers_json,
                request_body,
                response_headers_json,
                response_body,
                entry.error,
                entry.env,
                truncated as i64,
                entry.passed,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// List entries matching `filter`, newest first.
    pub fn list(&self, filter: &HistoryFilter) -> HistoryResult<Vec<HistorySummary>> {
        let mut sql = String::from(
            "SELECT id, executed_at, request_id, name, method, url, status, duration_ms, error, passed
             FROM entries WHERE 1=1",
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(text) = filter.text.as_ref().filter(|t| !t.is_empty()) {
            sql.push_str(" AND (name LIKE ?1 ESCAPE '\\' OR url LIKE ?1 ESCAPE '\\')");
            values.push(Box::new(like_pattern(text)));
        }
        if let Some(method) = &filter.method {
            sql.push_str(&format!(" AND method = ?{}", values.len() + 1));
            values.push(Box::new(method.clone()));
        }
        if let Some(min) = filter.status_min {
            sql.push_str(&format!(" AND status >= ?{}", values.len() + 1));
            values.push(Box::new(min));
        }
        if let Some(max) = filter.status_max {
            sql.push_str(&format!(" AND status <= ?{}", values.len() + 1));
            values.push(Box::new(max));
        }
        if let Some(request_id) = &filter.request_id {
            sql.push_str(&format!(" AND request_id = ?{}", values.len() + 1));
            values.push(Box::new(request_id.clone()));
        }

        sql.push_str(" ORDER BY id DESC");
        sql.push_str(&format!(" LIMIT ?{}", values.len() + 1));
        values.push(Box::new(filter.limit as i64));
        sql.push_str(&format!(" OFFSET ?{}", values.len() + 1));
        values.push(Box::new(filter.offset as i64));

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(HistorySummary {
                id: row.get(0)?,
                executed_at: row.get(1)?,
                request_id: row.get(2)?,
                name: row.get(3)?,
                method: row.get(4)?,
                url: row.get(5)?,
                status: row.get::<_, Option<i64>>(6)?.map(|s| s as u16),
                duration_ms: row.get(7)?,
                error: row.get(8)?,
                passed: row.get(9)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Fetch a full entry (with bodies) by id.
    pub fn get(&self, id: i64) -> HistoryResult<Option<HistoryEntry>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id, executed_at, request_id, name, method, url, status, duration_ms,
                        request_headers, request_body, response_headers, response_body,
                        error, env, truncated, passed
                 FROM entries WHERE id = ?1",
                params![id],
                row_to_entry,
            )
            .optional()?;
        match row {
            Some(Ok(entry)) => Ok(Some(entry)),
            Some(Err(err)) => Err(err),
            None => Ok(None),
        }
    }

    /// Delete a single entry by id.
    pub fn delete(&self, id: i64) -> HistoryResult<()> {
        self.conn()
            .execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete all entries.
    pub fn clear(&self) -> HistoryResult<()> {
        self.conn().execute("DELETE FROM entries", [])?;
        Ok(())
    }

    /// Keep only the newest `max_entries` rows, deleting the rest.
    pub fn prune(&self, max_entries: usize) -> HistoryResult<usize> {
        let conn = self.conn();
        let deleted = conn.execute(
            "DELETE FROM entries WHERE id NOT IN (
                SELECT id FROM entries ORDER BY id DESC LIMIT ?1
            )",
            params![max_entries as i64],
        )?;
        Ok(deleted)
    }

    /// Total number of stored entries.
    pub fn count(&self) -> HistoryResult<usize> {
        let conn = self.conn();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryResult<HistoryEntry>> {
    let request_headers_json: String = row.get(8)?;
    let response_headers_json: String = row.get(10)?;
    let truncated: i64 = row.get(14)?;

    let entry = (|| -> HistoryResult<HistoryEntry> {
        Ok(HistoryEntry {
            id: row.get(0)?,
            executed_at: row.get(1)?,
            request_id: row.get(2)?,
            name: row.get(3)?,
            method: row.get(4)?,
            url: row.get(5)?,
            status: row.get::<_, Option<i64>>(6)?.map(|s| s as u16),
            duration_ms: row.get(7)?,
            request_headers: serde_json::from_str(&request_headers_json)?,
            request_body: row.get(9)?,
            response_headers: serde_json::from_str(&response_headers_json)?,
            response_body: row.get(11)?,
            error: row.get(12)?,
            env: row.get(13)?,
            passed: row.get(15)?,
            truncated: truncated != 0,
        })
    })();
    Ok(entry)
}

fn init_schema(conn: &Connection) -> HistoryResult<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < SCHEMA_VERSION {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                executed_at TEXT NOT NULL,
                request_id TEXT NOT NULL,
                name TEXT NOT NULL,
                method TEXT NOT NULL,
                url TEXT NOT NULL,
                status INTEGER,
                duration_ms INTEGER NOT NULL,
                request_headers TEXT NOT NULL,
                request_body BLOB,
                response_headers TEXT NOT NULL,
                response_body BLOB,
                error TEXT,
                env TEXT,
                truncated INTEGER NOT NULL DEFAULT 0,
                passed INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_entries_request_id ON entries(request_id);
            CREATE INDEX IF NOT EXISTS idx_entries_executed_at ON entries(executed_at);",
        )?;
        // v1 -> v2: the verdict column. Fresh databases already have it
        // from CREATE TABLE; existing ones gain it here.
        match conn.execute("ALTER TABLE entries ADD COLUMN passed INTEGER", []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("duplicate column") => {}
            Err(error) => return Err(error.into()),
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

/// Truncate `data` to `MAX_STORED_BODY_BYTES`, returning whether it was cut.
fn cap_body(data: Option<Vec<u8>>) -> (Option<Vec<u8>>, bool) {
    match data {
        None => (None, false),
        Some(mut bytes) => {
            if bytes.len() > MAX_STORED_BODY_BYTES {
                bytes.truncate(MAX_STORED_BODY_BYTES);
                (Some(bytes), true)
            } else {
                (Some(bytes), false)
            }
        }
    }
}

/// Escape `%`, `_` and `\` then wrap in `%...%` for a `LIKE ... ESCAPE '\'` clause.
fn like_pattern(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}
