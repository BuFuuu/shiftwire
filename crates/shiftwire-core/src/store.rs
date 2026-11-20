//! SQLite storage, one file per project.
//!
//! Captures go through a bounded queue to a single writer thread that batches
//! inserts into one transaction every ~20 ms. The proxy never waits on disk: if
//! the queue is full, the capture is dropped and counted, never the request.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::events::{Bus, Event};
use crate::filter::Filter;
use crate::model::{Flow, FlowSummary, McpCall, RepeaterTab, TimelineEntry};
use crate::sender::Target;

const QUEUE_LEN: usize = 10_000;
const BATCH_WINDOW: Duration = Duration::from_millis(20);
const MAX_BATCH: usize = 500;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS flows (
    id TEXT PRIMARY KEY,           -- ULID: sorts by time
    ts_start INTEGER NOT NULL,     -- unix ms
    ts_end INTEGER NOT NULL,
    actor TEXT NOT NULL,
    source TEXT NOT NULL,
    scheme TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    query TEXT,
    http_version TEXT NOT NULL,
    status INTEGER,
    mime TEXT,
    req_head TEXT NOT NULL,
    req_body BLOB NOT NULL,
    req_size INTEGER NOT NULL,
    req_trunc INTEGER NOT NULL,
    resp_head TEXT NOT NULL,
    resp_body BLOB NOT NULL,
    resp_size INTEGER NOT NULL,
    resp_trunc INTEGER NOT NULL,
    error TEXT,
    color TEXT,
    note TEXT,
    seq INTEGER,                   -- the # in History: numbered when the request starts
    intent TEXT                    -- the agent's log_intent id it was sent under
);
CREATE INDEX IF NOT EXISTS flows_host ON flows(host);
CREATE INDEX IF NOT EXISTS flows_status ON flows(status);
CREATE INDEX IF NOT EXISTS flows_actor ON flows(actor);
CREATE TABLE IF NOT EXISTS tags (
    flow_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    actor TEXT NOT NULL,           -- who tagged it: 'human' or an agent name
    ts INTEGER NOT NULL,
    PRIMARY KEY (flow_id, tag)
);
CREATE INDEX IF NOT EXISTS tags_tag ON tags(tag);
CREATE TABLE IF NOT EXISTS timeline (
    id TEXT PRIMARY KEY,           -- ULID: time order
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,            -- intent | mark | operator
    text TEXT NOT NULL,
    flow_id TEXT,
    severity TEXT
);
CREATE INDEX IF NOT EXISTS timeline_flow ON timeline(flow_id) WHERE flow_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS mcp_calls (
    id TEXT PRIMARY KEY,           -- ULID: time order
    ts INTEGER NOT NULL,
    agent TEXT NOT NULL,
    method TEXT NOT NULL,
    tool TEXT,
    request TEXT NOT NULL,
    response TEXT NOT NULL,
    is_error INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    color TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS repeater_tabs (
    id TEXT PRIMARY KEY,           -- ULID: tabs keep their creation order
    name TEXT NOT NULL,
    scheme TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    raw TEXT NOT NULL,
    sends TEXT NOT NULL            -- flow ids, comma-separated (ULIDs have no commas)
);
";

const SUMMARY_COLS: &str = "seq, intent, id, ts_start, ts_end, actor, source, method, scheme, host, port, path, query,
    status, mime, resp_size, error, color, note,
    (SELECT severity FROM timeline WHERE timeline.flow_id = flows.id AND kind = 'mark' ORDER BY id DESC LIMIT 1) AS mark,
    (SELECT text FROM timeline WHERE timeline.flow_id = flows.id AND kind = 'mark' ORDER BY id DESC LIMIT 1) AS mark_reason,
    (SELECT group_concat(tag, char(31)) FROM tags WHERE tags.flow_id = flows.id) AS tag_list";

pub(crate) struct Store {
    /// Reads and small metadata writes (tags, colors, notes); captures use the writer thread.
    conn: Mutex<Connection>,
    queue: Mutex<Option<SyncSender<Flow>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    count: Arc<AtomicU64>,
    dropped: AtomicU64,
    /// The newest `#` handed out.
    last_seq: AtomicU64,
}

impl Store {
    pub fn open(path: &Path, bus: Bus) -> rusqlite::Result<Self> {
        let write = connect(path)?;
        write.execute_batch(SCHEMA)?;
        migrate(&write)?;
        let conn = connect(path)?;
        let last_seq: i64 =
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM flows", [], |r| r.get(0))?;
        let existing: i64 = conn.query_row("SELECT COUNT(*) FROM flows", [], |r| r.get(0))?;
        let count = Arc::new(AtomicU64::new(existing as u64));

        let (tx, rx) = sync_channel(QUEUE_LEN);
        let writer_count = count.clone();
        let writer = std::thread::Builder::new()
            .name("shiftwire-store".into())
            .spawn(move || write_loop(write, rx, bus, writer_count))
            .expect("spawn store writer");

        Ok(Self {
            conn: Mutex::new(conn),
            queue: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            count,
            dropped: AtomicU64::new(0),
            last_seq: AtomicU64::new(last_seq as u64),
        })
    }

    /// Queues a capture. Never blocks.
    /// The next `#`. Taken when a request starts, so numbers follow start order
    /// even when a slow request finishes after later ones.
    pub fn next_seq(&self) -> u64 {
        self.last_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn record(&self, mut flow: Flow) {
        if flow.seq == 0 {
            flow.seq = self.next_seq();
        }
        let queue = self.queue.lock().expect("store queue lock");
        let queued = queue.as_ref().is_some_and(|q| q.try_send(flow).is_ok());
        if !queued {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The newest `limit` flows matching `filter`, oldest first (ready to append in a list).
    pub fn query(&self, filter: &Filter, limit: usize) -> rusqlite::Result<Vec<FlowSummary>> {
        self.query_page(filter, limit, None)
    }

    /// Like [`Store::query`], but only flows older than `before` (an id): the
    /// next page when scrolling back in time.
    /// Ids of flows matching `filter`, newest first (for bulk tagging and notes).
    pub fn ids(&self, filter: &Filter, limit: usize) -> rusqlite::Result<Vec<String>> {
        let (where_sql, filter_params) = filter.to_sql();
        let mut params: Vec<Value> = filter_params.into_iter().map(Value::Text).collect();
        params.push(Value::Integer(limit as i64));
        let sql = format!(
            "SELECT id FROM flows WHERE {where_sql} ORDER BY id DESC LIMIT ?{}",
            params.len()
        );
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&sql)?;
        let ids = stmt
            .query_map(params_from_iter(params), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }

    pub fn query_page(
        &self,
        filter: &Filter,
        limit: usize,
        before: Option<&str>,
    ) -> rusqlite::Result<Vec<FlowSummary>> {
        let (mut where_sql, filter_params) = filter.to_sql();
        let mut params: Vec<Value> = filter_params.into_iter().map(Value::Text).collect();
        if let Some(id) = before {
            params.push(Value::Text(id.to_string()));
            where_sql = format!("({where_sql}) AND id < ?{}", params.len());
        }
        params.push(Value::Integer(limit as i64));
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM flows WHERE {where_sql} ORDER BY id DESC LIMIT ?{}",
            params.len()
        );
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows: Vec<FlowSummary> = stmt
            .query_map(params_from_iter(params), |r| {
                summary_row(r).map(|f| f.summary())
            })?
            .collect::<Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// How many flows match `filter` (for "2,000 of 41,230" in the UI).
    pub fn count_matching(&self, filter: &Filter) -> rusqlite::Result<u64> {
        if filter.is_empty() {
            return Ok(self.count());
        }
        let (where_sql, params) = filter.to_sql();
        let conn = self.conn.lock().expect("store lock");
        let n: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM flows WHERE {where_sql}"),
            params_from_iter(params),
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    pub fn summaries(&self, ids: &[String]) -> rusqlite::Result<Vec<FlowSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM flows WHERE id IN ({}) ORDER BY id",
            placeholders(ids.len(), 1)
        );
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(ids), |r| {
            summary_row(r).map(|f| f.summary())
        })?
        .collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Flow>> {
        let conn = self.conn.lock().expect("store lock");
        let sql = format!(
            "SELECT {SUMMARY_COLS}, http_version, req_head, req_body, req_size, req_trunc,
                    resp_head, resp_body, resp_trunc FROM flows WHERE id = ?1"
        );
        conn.query_row(&sql, [id], |r| {
            let mut f = summary_row(r)?;
            f.http_version = r.get("http_version")?;
            f.req_head = r.get("req_head")?;
            f.req_body = r.get("req_body")?;
            f.req_size = int(r, "req_size")?;
            f.req_trunc = r.get("req_trunc")?;
            f.resp_head = r.get("resp_head")?;
            f.resp_body = r.get("resp_body")?;
            f.resp_trunc = r.get("resp_trunc")?;
            Ok(f)
        })
        .optional()
    }

    pub fn tag(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
        actor: &str,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO tags (flow_id, tag, actor, ts) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut delete = tx.prepare("DELETE FROM tags WHERE flow_id = ?1 AND tag = ?2")?;
            let now = crate::now_ms() as i64;
            for id in ids {
                for tag in add {
                    insert.execute(params![id, tag, actor, now])?;
                }
                for tag in remove {
                    delete.execute(params![id, tag])?;
                }
            }
        }
        tx.commit()
    }

    /// Sets `column` (`color` or `note`) on every flow in `ids`; `None` clears it.
    pub fn set_field(
        &self,
        ids: &[String],
        column: &str,
        value: Option<&str>,
    ) -> rusqlite::Result<()> {
        assert!(matches!(column, "color" | "note"), "not an editable column");
        let sql = format!(
            "UPDATE flows SET {column} = ?1 WHERE id IN ({})",
            placeholders(ids.len(), 2)
        );
        let mut params: Vec<Value> = vec![value.map_or(Value::Null, |v| Value::Text(v.into()))];
        params.extend(ids.iter().map(|id| Value::Text(id.clone())));
        self.conn
            .lock()
            .expect("store lock")
            .execute(&sql, params_from_iter(params))?;
        Ok(())
    }

    pub fn add_timeline(&self, e: &TimelineEntry) -> rusqlite::Result<()> {
        self.conn.lock().expect("store lock").execute(
            "INSERT INTO timeline (id, ts, actor, kind, text, flow_id, severity) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![e.id, e.ts as i64, e.actor, e.kind, e.text, e.flow_id, e.severity],
        )?;
        Ok(())
    }

    /// The newest `limit` entries (optionally only those after `after`), oldest first.
    pub fn timeline(
        &self,
        limit: usize,
        kind: Option<&str>,
        after: Option<&str>,
    ) -> rusqlite::Result<Vec<TimelineEntry>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT id, ts, actor, kind, text, flow_id, severity FROM timeline
             WHERE (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR id > ?2)
             ORDER BY id DESC LIMIT ?3",
        )?;
        let mut rows: Vec<TimelineEntry> = stmt
            .query_map(params![kind, after, limit as i64], |r| {
                Ok(TimelineEntry {
                    id: r.get(0)?,
                    ts: r.get::<_, i64>(1)? as u64,
                    actor: r.get(2)?,
                    kind: r.get(3)?,
                    text: r.get(4)?,
                    flow_id: r.get(5)?,
                    severity: r.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Raw material for the site map: requests grouped by host, path and method.
    pub fn site_groups(&self, host: Option<&str>) -> rusqlite::Result<Vec<crate::sitemap::Group>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT scheme, host, port, path, method, COUNT(*),
                SUM(status BETWEEN 200 AND 299), SUM(status BETWEEN 300 AND 399),
                SUM(status BETWEEN 400 AND 499), SUM(status >= 500), SUM(status IS NULL),
                SUM(actor NOT IN ('proxy', 'human')),
                0,
                MAX(ts_start),
                SUM(status = 404)
             FROM flows
             WHERE method != 'CONNECT' AND (?1 IS NULL OR host = ?1)
             GROUP BY scheme, host, port, path, method",
        )?;
        let n = |r: &Row, i: usize| -> rusqlite::Result<u64> {
            Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0) as u64)
        };
        let groups = stmt
            .query_map([host], |r| {
                Ok(crate::sitemap::Group {
                    scheme: r.get(0)?,
                    host: r.get(1)?,
                    port: r.get(2)?,
                    path: r.get(3)?,
                    method: r.get(4)?,
                    count: n(r, 5)?,
                    ok: n(r, 6)?,
                    redirect: n(r, 7)?,
                    client_error: n(r, 8)?,
                    server_error: n(r, 9)?,
                    failed: n(r, 10)?,
                    by_agents: n(r, 11)?,
                    marked: n(r, 12)? > 0,
                    last_seen: n(r, 13)?,
                    not_found: n(r, 14)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Marks are few: look them up once instead of probing for every request.
        let mut marked = conn.prepare(
            "SELECT DISTINCT f.scheme, f.host, f.port, f.path, f.method FROM timeline t
             JOIN flows f ON f.id = t.flow_id WHERE t.kind = 'mark'",
        )?;
        let marked: std::collections::HashSet<(String, String, u16, String, String)> = marked
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut groups = groups;
        for g in &mut groups {
            g.marked = marked.contains(&(
                g.scheme.clone(),
                g.host.clone(),
                g.port,
                g.path.clone(),
                g.method.clone(),
            ));
        }
        Ok(groups)
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .lock()
            .expect("store lock")
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("store lock").execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn add_mcp_call(&self, c: &McpCall) -> rusqlite::Result<()> {
        self.conn.lock().expect("store lock").execute(
            "INSERT INTO mcp_calls (id, ts, agent, method, tool, request, response, is_error, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                c.id,
                c.ts as i64,
                c.agent,
                c.method,
                c.tool,
                c.request,
                c.response,
                c.is_error,
                c.duration_ms as i64
            ],
        )?;
        Ok(())
    }

    /// The newest `limit` MCP calls older than `before` (if given), oldest first, with
    /// request and response cut to `preview` characters; or one call (`id`) in full.
    pub fn mcp_calls(
        &self,
        limit: usize,
        before: Option<&str>,
        preview: Option<usize>,
        id: Option<&str>,
    ) -> rusqlite::Result<Vec<McpCall>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT id, ts, agent, method, tool, is_error, duration_ms,
                    CASE WHEN ?3 IS NULL THEN request ELSE substr(request, 1, ?3) END,
                    CASE WHEN ?3 IS NULL THEN response ELSE substr(response, 1, ?3) END,
                    length(request), length(response), color
             FROM mcp_calls WHERE (?2 IS NULL OR id < ?2) AND (?4 IS NULL OR id = ?4)
             ORDER BY id DESC LIMIT ?1",
        )?;
        let preview = preview.map(|p| p as i64);
        let mut rows: Vec<McpCall> = stmt
            .query_map(params![limit as i64, before, preview, id], |r| {
                Ok(McpCall {
                    id: r.get(0)?,
                    ts: r.get::<_, i64>(1)? as u64,
                    agent: r.get(2)?,
                    method: r.get(3)?,
                    tool: r.get(4)?,
                    is_error: r.get(5)?,
                    duration_ms: r.get::<_, i64>(6)? as u64,
                    request: r.get(7)?,
                    response: r.get(8)?,
                    request_size: r.get::<_, i64>(9)? as u64,
                    response_size: r.get::<_, i64>(10)? as u64,
                    color: r.get(11)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn color_mcp_calls(&self, ids: &[String], color: Option<&str>) -> rusqlite::Result<()> {
        let sql = format!(
            "UPDATE mcp_calls SET color = ?1 WHERE id IN ({})",
            placeholders(ids.len(), 2)
        );
        let mut params: Vec<Value> = vec![color.map_or(Value::Null, |c| Value::Text(c.into()))];
        params.extend(ids.iter().map(|id| Value::Text(id.clone())));
        self.conn
            .lock()
            .expect("store lock")
            .execute(&sql, params_from_iter(params))?;
        Ok(())
    }

    pub fn repeater_tabs(&self) -> rusqlite::Result<Vec<RepeaterTab>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(
            "SELECT id, name, scheme, host, port, raw, sends FROM repeater_tabs ORDER BY id",
        )?;
        stmt.query_map([], |r| {
            let sends: String = r.get("sends")?;
            Ok(RepeaterTab {
                id: r.get("id")?,
                name: r.get("name")?,
                target: Target {
                    scheme: r.get("scheme")?,
                    host: r.get("host")?,
                    port: r.get("port")?,
                },
                raw: r.get("raw")?,
                sends: sends
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })?
        .collect()
    }

    pub fn save_repeater_tab(&self, tab: &RepeaterTab) -> rusqlite::Result<()> {
        self.conn.lock().expect("store lock").execute(
            "INSERT OR REPLACE INTO repeater_tabs (id, name, scheme, host, port, raw, sends)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                tab.id,
                tab.name,
                tab.target.scheme,
                tab.target.host,
                tab.target.port,
                tab.raw,
                tab.sends.join(",")
            ],
        )?;
        Ok(())
    }

    pub fn delete_repeater_tab(&self, id: &str) -> rusqlite::Result<()> {
        self.conn
            .lock()
            .expect("store lock")
            .execute("DELETE FROM repeater_tabs WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Writes everything still queued, then stops the writer thread.
    pub fn close(&self) {
        drop(self.queue.lock().expect("store queue lock").take());
        if let Some(writer) = self.writer.lock().expect("store writer lock").take() {
            let _ = writer.join();
        }
    }
}

/// Number of flows in the database at `path` without opening it for writing
/// (for listing projects that are not loaded). `None` if there is none yet.
pub fn count_in(path: &Path) -> Option<u64> {
    let conn =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM flows", [], |r| r.get(0))
        .ok()?;
    Some(n as u64)
}

/// Databases from before `#` numbers get them once, in start (id) order.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let has_seq = conn
        .prepare("SELECT 1 FROM pragma_table_info('flows') WHERE name = 'seq'")?
        .exists([])?;
    if !has_seq {
        conn.execute_batch(
            "ALTER TABLE flows ADD COLUMN seq INTEGER;
             UPDATE flows SET seq = r.n
               FROM (SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS n FROM flows) AS r
              WHERE r.id = flows.id;",
        )?;
    }
    let has_intent = conn
        .prepare("SELECT 1 FROM pragma_table_info('flows') WHERE name = 'intent'")?
        .exists([])?;
    if !has_intent {
        conn.execute_batch("ALTER TABLE flows ADD COLUMN intent TEXT;")?;
    }
    let has_call_color = conn
        .prepare("SELECT 1 FROM pragma_table_info('mcp_calls') WHERE name = 'color'")?
        .exists([])?;
    if !has_call_color {
        conn.execute_batch("ALTER TABLE mcp_calls ADD COLUMN color TEXT;")?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS flows_seq ON flows(seq);")
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
    )?;
    Ok(conn)
}

fn write_loop(mut conn: Connection, rx: Receiver<Flow>, bus: Bus, count: Arc<AtomicU64>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + BATCH_WINDOW;
        while batch.len() < MAX_BATCH {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(flow) => batch.push(flow),
                Err(_) => break, // window over, or shutting down
            }
        }
        match insert(&mut conn, &batch) {
            Ok(()) => {
                count.fetch_add(batch.len() as u64, Ordering::Relaxed);
                for flow in &batch {
                    bus.emit(Event::FlowAdded {
                        flow: Box::new(flow.summary()),
                    });
                }
            }
            Err(e) => tracing::error!("store: could not write {} flows: {e}", batch.len()),
        }
    }
}

fn insert(conn: &mut Connection, batch: &[Flow]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO flows (id, ts_start, ts_end, actor, source, scheme, host, port, method, path,
                query, http_version, status, mime, req_head, req_body, req_size, req_trunc,
                resp_head, resp_body, resp_size, resp_trunc, error, seq, intent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        )?;
        for f in batch {
            stmt.execute(params![
                f.id,
                f.ts_start as i64,
                f.ts_end as i64,
                f.actor,
                f.source,
                f.scheme,
                f.host,
                f.port,
                f.method,
                f.path,
                f.query,
                f.http_version,
                f.status,
                f.mime,
                f.req_head,
                f.req_body,
                f.req_size as i64,
                f.req_trunc,
                f.resp_head,
                f.resp_body,
                f.resp_size as i64,
                f.resp_trunc,
                f.error,
                f.seq as i64,
                f.intent,
            ])?;
            // Tags set while capturing, e.g. by match & replace / alert rules.
            for tag in &f.tags {
                tx.execute(
                    "INSERT OR IGNORE INTO tags (flow_id, tag, actor, ts) VALUES (?1, ?2, 'rules', ?3)",
                    params![f.id, tag, f.ts_start as i64],
                )?;
            }
        }
    }
    tx.commit()
}

/// SQLite integers are i64; sizes and timestamps are never negative.
fn int(r: &Row, column: &str) -> rusqlite::Result<u64> {
    Ok(r.get::<_, i64>(column)? as u64)
}

/// `?first, ?first+1, ...` for `n` values.
fn placeholders(n: usize, first: usize) -> String {
    (first..first + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tags are joined with the ASCII unit separator, which never appears in a tag.
const TAG_SEPARATOR: char = '\x1f';

/// Maps the `SUMMARY_COLS` part of a row.
fn summary_row(r: &Row) -> rusqlite::Result<Flow> {
    let tags: Option<String> = r.get("tag_list")?;
    Ok(Flow {
        id: r.get("id")?,
        seq: int(r, "seq")?,
        ts_start: int(r, "ts_start")?,
        ts_end: int(r, "ts_end")?,
        actor: r.get("actor")?,
        source: r.get("source")?,
        method: r.get("method")?,
        scheme: r.get("scheme")?,
        host: r.get("host")?,
        port: r.get("port")?,
        path: r.get("path")?,
        query: r.get("query")?,
        status: r.get("status")?,
        mime: r.get("mime")?,
        resp_size: int(r, "resp_size")?,
        error: r.get("error")?,
        color: r.get("color")?,
        note: r.get("note")?,
        mark: r.get("mark")?,
        mark_reason: r.get("mark_reason")?,
        intent: r.get("intent")?,
        tags: tags
            .map(|t| t.split(TAG_SEPARATOR).map(str::to_string).collect())
            .unwrap_or_default(),
        ..Flow::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(n: u64) -> Flow {
        Flow {
            id: crate::new_id(),
            ts_start: n,
            ts_end: n + 5,
            actor: "proxy".into(),
            source: "proxy".into(),
            scheme: "http".into(),
            host: "example.com".into(),
            port: 80,
            method: "GET".into(),
            path: format!("/{n}"),
            http_version: "HTTP/1.1".into(),
            status: Some(200),
            resp_body: b"hello".to_vec(),
            resp_size: 5,
            ..Flow::default()
        }
    }

    #[tokio::test]
    async fn writes_persist_and_emit_events() {
        let path = std::env::temp_dir().join(format!("shiftwire-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        let store = Store::open(&path, bus).unwrap();

        let first = flow(1);
        let id = first.id.clone();
        store.record(first);
        std::thread::sleep(Duration::from_millis(2));
        store.record(flow(2));
        store.close(); // flushes

        assert_eq!(store.count(), 2);
        assert!(matches!(rx.recv().await, Ok(Event::FlowAdded { .. })));
        let recent = store.query(&Filter::default(), 10).unwrap();
        assert_eq!(
            recent.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["/1", "/2"]
        );
        let full = store.get(&id).unwrap().unwrap();
        assert_eq!(full.resp_body, b"hello");
        assert_eq!(full.duration_ms(), 5);

        // Reopening sees the persisted rows.
        drop(store);
        let again = Store::open(&path, Bus::new()).unwrap();
        assert_eq!(again.count(), 2);
        again.close();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn numbers_follow_start_order_and_old_databases_get_them() {
        let path =
            std::env::temp_dir().join(format!("shiftwire-store-seq-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // A database from before `#` numbers: no seq column yet.
        let old = Connection::open(&path).unwrap();
        old.execute_batch(&SCHEMA.replace(",\n    seq INTEGER", ""))
            .unwrap();
        for id in ["01B", "01A"] {
            old.execute(
                "INSERT INTO flows (id, ts_start, ts_end, actor, source, scheme, host, port, method,
                    path, http_version, req_head, req_body, req_size, req_trunc, resp_head,
                    resp_body, resp_size, resp_trunc)
                 VALUES (?1, 0, 0, 'proxy', 'proxy', 'http', 'a', 80, 'GET', '/', 'HTTP/1.1', '',
                    x'', 0, 0, '', x'', 0, 0)",
                [id],
            )
            .unwrap();
        }
        drop(old);

        let store = Store::open(&path, Bus::new()).unwrap();
        let (slow, fast) = (store.next_seq(), store.next_seq());
        assert_eq!((slow, fast), (3, 4), "continues after the migrated rows");
        let mut started_first = flow(1);
        started_first.seq = slow;
        let mut started_second = flow(2);
        started_second.seq = fast;
        store.record(started_second); // finishes first
        store.record(started_first);
        store.close();

        let all = store.query(&Filter::default(), 10).unwrap();
        let numbers: Vec<_> = all.iter().map(|f| (f.id.as_str(), f.seq)).collect();
        assert_eq!(
            &numbers[..2],
            [("01A", 1), ("01B", 2)],
            "migrated in id order"
        );
        assert_eq!(all[2].seq, 3, "the request that started first keeps #3");
        assert_eq!(all[3].seq, 4);
    }

    #[test]
    fn record_after_close_counts_as_dropped() {
        let path =
            std::env::temp_dir().join(format!("shiftwire-store-drop-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path, Bus::new()).unwrap();
        store.close();
        store.record(flow(1));
        assert_eq!(store.dropped(), 1);
        let _ = std::fs::remove_file(&path);
    }

    /// The UI filters live events in memory and history in SQL: both must agree.
    #[test]
    #[allow(clippy::type_complexity)] // a compact table of test rows
    fn sql_and_live_filters_agree() {
        let path =
            std::env::temp_dir().join(format!("shiftwire-store-filter-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path, Bus::new()).unwrap();
        let mut ids = Vec::new();
        let specs: [(
            &str,
            &str,
            Option<&str>,
            &str,
            Option<u16>,
            u64,
            u64,
            Option<&str>,
            Option<&str>,
        ); 8] = [
            (
                "shop.test",
                "/login",
                None,
                "proxy",
                Some(200),
                512,
                40,
                Some("text/html"),
                None,
            ),
            (
                "api.shop.test",
                "/users/7",
                Some("admin=1"),
                "claude",
                Some(403),
                2048,
                150,
                Some("application/json"),
                None,
            ),
            (
                "api.shop.test",
                "/orders",
                Some("id=5"),
                "claude",
                Some(500),
                90,
                900,
                Some("application/json"),
                None,
            ),
            (
                "cdn.other.test",
                "/50%_off.png",
                None,
                "proxy",
                Some(304),
                0,
                5,
                Some("image/png"),
                None,
            ),
            (
                "evil.test",
                "",
                None,
                "proxy",
                None,
                0,
                30,
                None,
                Some("refused"),
            ),
            (
                "SHOP.test",
                "/Search",
                Some("q=Admin"),
                "ffuf",
                Some(404),
                1500,
                99,
                None,
                None,
            ),
            (
                "a.test",
                "/x",
                None,
                "claude",
                Some(201),
                1024,
                100,
                Some("text/plain"),
                None,
            ),
            (
                "tunnel.test",
                "",
                None,
                "proxy",
                Some(200),
                7000,
                4000,
                None,
                None,
            ),
        ];
        for (n, (host, p, q, actor, status, size, ms, mime, error)) in specs.into_iter().enumerate()
        {
            let mut f = flow(n as u64);
            (f.host, f.path, f.query, f.actor) =
                (host.into(), p.into(), q.map(Into::into), actor.into());
            (f.status, f.resp_size, f.ts_end, f.mime, f.error) = (
                status,
                size,
                f.ts_start + ms,
                mime.map(Into::into),
                error.map(Into::into),
            );
            if host == "tunnel.test" {
                f.method = "CONNECT".into();
                f.source = "tunnel".into();
            }
            if n == 2 {
                f.method = "POST".into();
                f.source = "mcp".into();
            }
            if n == 3 {
                f.source = "invisible:localhost:8123".into();
            }
            if n == 4 {
                f.source = "redirect".into(); // an early invisible-proxy build
            }
            ids.push(f.id.clone());
            store.record(f);
            std::thread::sleep(Duration::from_millis(2)); // distinct ULID order
        }
        store.close();
        store
            .tag(
                &ids[1..3],
                &["idor".into(), "auth-bypass".into()],
                &[],
                "claude",
            )
            .unwrap();
        store
            .tag(&ids[5..6], &["idor".into()], &[], "human")
            .unwrap();
        store.set_field(&ids[1..2], "color", Some("red")).unwrap();
        store
            .set_field(&ids[6..7], "note", Some("check this later"))
            .unwrap();

        let mark = crate::model::TimelineEntry {
            id: crate::new_id(),
            ts: 1,
            actor: "claude".into(),
            kind: "mark".into(),
            text: "user 7 can read user 8".into(),
            flow_id: Some(ids[1].clone()),
            severity: Some("high".into()),
        };
        store.add_timeline(&mark).unwrap();

        let all = store.query(&Filter::default(), 1000).unwrap();
        assert_eq!(all.len(), 8);
        assert_eq!(all[1].mark.as_deref(), Some("high"));
        assert_eq!(
            all[1].mark_reason.as_deref(),
            Some("user 7 can read user 8")
        );
        for text in [
            "",
            "host:shop",
            "host:*.shop.test",
            "host:=shop.test",
            "status:4xx",
            "status:0",
            "-status:200",
            "status:>=400 status:<500",
            "size:>1k",
            "size:<=1k",
            "ms:>=100",
            "tag:idor",
            "tag:auth*",
            "tag:auth",
            "is:tagged",
            "-is:tagged",
            "color:red",
            "-color:red",
            "is:noted",
            "note:later",
            "is:error",
            "is:ai",
            "actor:claude",
            "method:post",
            "method:CONNECT",
            "is:tunnel",
            "mime:json",
            "admin",
            "\"50%_off\"",
            "(host:a.test OR tag:idor) -is:error",
            "path:id=",
            "search",
            "NOT (is:ai OR is:error)",
            "443",
            "ffuf",
            "404",
            "post",
            "shop.test:443",
            "claude 500",
            "is:marked",
            "-is:marked",
            "id:>4",
            "id:<=2",
            "status:2*",
            "intent:01ABC",
            "-intent:01ABC",
            "via:mcp",
            "via:invisible",
            "-via:proxy",
            "via:tun*",
            "idor",
            "auth*",
            "-idor",
            "status:*0*",
            "-status:4*",
        ] {
            let filter = Filter::parse(text).unwrap();
            let sql: Vec<_> = store
                .query(&filter, 1000)
                .unwrap()
                .into_iter()
                .map(|f| f.id)
                .collect();
            let live: Vec<_> = all
                .iter()
                .filter(|f| filter.matches(f))
                .map(|f| f.id.clone())
                .collect();
            assert_eq!(sql, live, "filter {text:?}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pages_walk_back_in_time_and_counts_match() {
        let path =
            std::env::temp_dir().join(format!("shiftwire-store-page-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path, Bus::new()).unwrap();
        for n in 0..5 {
            let mut f = flow(n);
            f.status = Some(if n % 2 == 0 { 200 } else { 404 });
            store.record(f);
            std::thread::sleep(Duration::from_millis(2));
        }
        store.close();
        let all = Filter::default();
        let newest = store.query_page(&all, 2, None).unwrap();
        assert_eq!(
            newest.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["/3", "/4"]
        );
        let older = store.query_page(&all, 2, Some(&newest[0].id)).unwrap();
        assert_eq!(
            older.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["/1", "/2"]
        );
        let not_found = Filter::parse("status:404").unwrap();
        assert_eq!(store.count_matching(&not_found).unwrap(), 2);
        assert_eq!(store.count_matching(&all).unwrap(), 5);
        let _ = std::fs::remove_file(&path);
    }
}
