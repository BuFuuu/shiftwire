//! `shiftwire mcp`: MCP over stdio for clients that only speak stdio.
//!
//! Each JSON-RPC line from stdin is forwarded to the daemon's MCP port, and the
//! reply goes to stdout. Starts the daemon if it is not running. Nothing but
//! protocol messages is ever written to stdout (diagnostics go to stderr).

use std::io::{self, BufRead, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::client;
use crate::daemon::{self, DaemonState, Paths};

pub fn run(paths: &Paths, agent: Option<&str>) -> Result<()> {
    let state = ensure_daemon(paths)?;
    let url = match agent {
        Some(a) => format!("{}/mcp?agent={}", state.mcp_url, a.replace(' ', "%20")),
        None => format!("{}/mcp", state.mcp_url),
    };
    let mut session: Option<String> = None;
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut request = ureq::post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .timeout(Duration::from_secs(120)); // send_request may wait up to 30 s per request
        if let Some(s) = &session {
            request = request.set("Mcp-Session-Id", s);
        }
        match request.send_string(&line) {
            Ok(resp) => {
                if let Some(s) = resp.header("mcp-session-id") {
                    session = Some(s.to_string());
                }
                let body = resp.into_string()?;
                if !body.trim().is_empty() {
                    writeln!(out, "{}", body.trim())?;
                    out.flush()?;
                }
            }
            Err(e) => {
                eprintln!("shiftwire mcp: {e}");
                // Answer requests (not notifications) so the client does not hang.
                if let Some(id) = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                {
                    let reply = json!({ "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32603, "message": format!("shiftWire daemon unreachable: {e}") } });
                    writeln!(out, "{reply}")?;
                    out.flush()?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_daemon(paths: &Paths) -> Result<DaemonState> {
    if daemon::is_running(paths)
        && let Some(state) = DaemonState::read(paths)
    {
        return Ok(state);
    }
    eprintln!("shiftwire mcp: starting the shiftWire daemon…");
    let _ = std::fs::remove_file(&paths.state);
    daemon::spawn_detached(paths)?;
    daemon::wait_until(Duration::from_secs(5), || {
        DaemonState::read(paths).is_some_and(|s| client::health(&s.api_url))
    });
    DaemonState::read(paths).context("the shiftWire daemon did not start (see `shiftwire logs`)")
}
