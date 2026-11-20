//! Agents' browser tools. shiftWire runs Playwright MCP (Node, via `npx`) as a child
//! over stdio, attached to the AI browser's DevTools port, and offers an allow-list of
//! its tools on its own MCP port. One call at a time; the child is restarted when the
//! browser was reopened (new DevTools port).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// Pinned: tool names and arguments are part of what agents see.
const PACKAGE: &str = "@playwright/mcp@0.0.86";
const TIMEOUT: Duration = Duration::from_secs(120);

/// What agents may use. Left out: tools that read or write files on this machine
/// (`file_upload`, `drop`, `filename` arguments), run arbitrary code outside the page
/// (`run_code_unsafe`), duplicate History (network tools) or change the window itself.
const ALLOWED: [&str; 16] = [
    "browser_navigate",
    "browser_navigate_back",
    "browser_snapshot",
    "browser_find",
    "browser_click",
    "browser_type",
    "browser_fill_form",
    "browser_select_option",
    "browser_press_key",
    "browser_hover",
    "browser_wait_for",
    "browser_tabs",
    "browser_handle_dialog",
    "browser_console_messages",
    "browser_take_screenshot",
    "browser_evaluate",
];

/// Allowed in advise mode (plus `browser_tabs` with `action: list`): they only look.
pub const READ_ONLY: [&str; 4] = [
    "browser_snapshot",
    "browser_find",
    "browser_console_messages",
    "browser_take_screenshot",
];

pub fn allowed(name: &str) -> bool {
    ALLOWED.contains(&name)
}

pub struct Playwright {
    /// Where Playwright may put output files; `None` switches browser tools off (tests).
    output_dir: Option<PathBuf>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    child: Option<Running>,
    /// The tool list, fetched once (the package version is pinned).
    tools: Option<Vec<Value>>,
}

struct Running {
    cdp_port: u16,
    process: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Playwright {
    pub fn new(output_dir: Option<PathBuf>) -> Self {
        Self {
            output_dir,
            state: Mutex::default(),
        }
    }

    /// The allowed browser tools (without `filename` arguments), for `tools/list`.
    /// Works without a browser: Playwright connects only when a tool runs.
    pub async fn tools(&self, cdp_port: Option<u16>) -> Result<Vec<Value>, String> {
        let mut state = self.state.lock().await;
        if let Some(tools) = &state.tools {
            return Ok(tools.clone());
        }
        let child = self.child(&mut state, cdp_port.unwrap_or(1)).await?;
        let listed = child.rpc("tools/list", json!({})).await?;
        let tools: Vec<Value> = listed["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["name"].as_str().is_some_and(allowed))
            .map(|t| {
                let mut t = t.clone();
                if let Some(props) = t["inputSchema"]["properties"].as_object_mut() {
                    props.remove("filename");
                }
                t
            })
            .collect();
        state.tools = Some(tools.clone());
        Ok(tools)
    }

    /// Runs one browser tool; returns Playwright's `content` items and whether it failed.
    pub async fn call(
        &self,
        cdp_port: u16,
        name: &str,
        args: &Value,
    ) -> Result<(Vec<Value>, bool), String> {
        let mut args = args.clone();
        if let Some(map) = args.as_object_mut() {
            map.remove("filename");
        }
        let mut state = self.state.lock().await;
        let child = self.child(&mut state, cdp_port).await?;
        let result = child
            .rpc("tools/call", json!({ "name": name, "arguments": args }))
            .await;
        if result.is_err() {
            state.child = None; // a broken child is replaced on the next call
        }
        let result = result?;
        let content = result["content"].as_array().cloned().unwrap_or_default();
        Ok((content, result["isError"].as_bool().unwrap_or(false)))
    }

    /// The child attached to `cdp_port`, started (and initialized) if needed.
    async fn child<'a>(
        &self,
        state: &'a mut State,
        cdp_port: u16,
    ) -> Result<&'a mut Running, String> {
        let Some(output_dir) = &self.output_dir else {
            return Err("browser tools are switched off".into());
        };
        let alive = state
            .child
            .as_mut()
            .is_some_and(|c| c.cdp_port == cdp_port && matches!(c.process.try_wait(), Ok(None)));
        if !alive {
            state.child = None; // kill_on_drop ends the old one
            let mut process = Command::new("npx")
                .args(["-y", PACKAGE, "--cdp-endpoint"])
                .arg(format!("http://127.0.0.1:{cdp_port}"))
                .arg("--output-dir")
                .arg(output_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| {
                    format!("the browser tools need Node.js (npx), which could not be started: {e}")
                })?;
            let stdin = process.stdin.take().ok_or("no stdin")?;
            let stdout = BufReader::new(process.stdout.take().ok_or("no stdout")?).lines();
            let mut child = Running {
                cdp_port,
                process,
                stdin,
                stdout,
                next_id: 1,
            };
            let hello = json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "shiftwire", "version": shiftwire_core::VERSION },
            });
            child.rpc("initialize", hello).await?;
            child
                .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .await?;
            state.child = Some(child);
        }
        Ok(state.child.as_mut().expect("just set"))
    }
}

impl Running {
    async fn send(&mut self, msg: &Value) -> Result<(), String> {
        let line = format!("{msg}\n");
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Playwright MCP stopped: {e}"))
    }

    /// One JSON-RPC request; its notifications in between are skipped.
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        let reply = tokio::time::timeout(TIMEOUT, async {
            loop {
                let line = self
                    .stdout
                    .next_line()
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("Playwright MCP exited")?;
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if msg["id"] == json!(id) {
                    return Ok::<Value, String>(msg);
                }
            }
        })
        .await
        .map_err(|_| {
            format!(
                "Playwright MCP did not answer {method} within {} s",
                TIMEOUT.as_secs()
            )
        })??;
        match reply.get("error") {
            Some(error) => Err(format!(
                "Playwright MCP: {}",
                error["message"].as_str().unwrap_or("error")
            )),
            None => Ok(reply["result"].clone()),
        }
    }
}
