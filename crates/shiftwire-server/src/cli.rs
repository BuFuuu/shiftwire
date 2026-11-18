//! The user-facing commands: start, stop, status, open, logs.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::daemon::{self, DaemonState as State, Paths};
use crate::{client, open};

pub fn start(paths: &Paths) -> Result<State> {
    if daemon::is_running(paths) {
        let state = running_state(paths)?;
        println!("shiftWire is already running (pid {}).", state.pid);
        print_urls(&state);
        return Ok(state);
    }
    let _ = fs::remove_file(&paths.state); // stale file from a crash
    daemon::spawn_detached(paths)?;
    let up = daemon::wait_until(Duration::from_secs(5), || {
        State::read(paths).is_some_and(|s| client::health(&s.api_url))
    });
    if !up {
        bail!(
            "shiftWire did not come up within 5 s. Recent log:\n{}",
            tail(paths, 15)
        );
    }
    let state = running_state(paths)?;
    println!("shiftWire started in the background (pid {}).", state.pid);
    print_urls(&state);
    Ok(state)
}

pub fn stop(paths: &Paths) -> Result<()> {
    if !daemon::is_running(paths) {
        println!("shiftWire is not running.");
        return Ok(());
    }
    let state = running_state(paths)?;
    if let Err(e) = client::shutdown(&state.api_url, &state.token) {
        eprintln!("Graceful stop failed ({e}); killing pid {}.", state.pid);
        daemon::kill(state.pid)?;
    }
    if !daemon::wait_until(Duration::from_secs(10), || !daemon::is_running(paths)) {
        daemon::kill(state.pid)?;
        daemon::wait_until(Duration::from_secs(2), || !daemon::is_running(paths));
    }
    println!("shiftWire stopped.");
    Ok(())
}

pub fn status(paths: &Paths) -> Result<()> {
    if !daemon::is_running(paths) {
        println!("shiftWire is not running. Start it with `shiftwire start`.");
        return Ok(());
    }
    let state = running_state(paths)?;
    let s = client::status(&state.api_url).context("shiftWire is running but did not answer")?;
    let proxy = match (&s.proxy_error, s.proxy_running) {
        (Some(error), _) => format!("DOWN: {error}"),
        (None, true) => s.proxy_addr.clone(),
        (None, false) => format!("{} (starting)", s.proxy_addr),
    };
    println!(
        "● shiftWire {} running   pid {}   up {}",
        s.version,
        s.pid,
        human_duration(s.uptime_s)
    );
    println!("  project  {}   {}", s.project, s.project_path);
    print_urls(&state);
    if !s.api_addr.starts_with("127.") && !s.api_addr.starts_with("[::1]") {
        println!(
            "  listens  {} (UI also reachable from other machines)",
            s.api_addr
        );
    }
    println!("  proxy    {proxy}");
    println!(
        "  flows    {}   agents {}   agent mode: {}",
        s.flows,
        s.agents,
        s.agent_mode.as_str()
    );
    if s.dropped > 0 {
        println!(
            "  dropped  {} captures (store could not keep up; traffic was still forwarded)",
            s.dropped
        );
    }
    println!(
        "  CA       {}…  (trust it: http://shiftwire/ via the proxy)",
        &s.ca_fingerprint[..23]
    );
    for (name, label) in [("api.token", "UI token"), ("proxy.password", "proxy pw")] {
        if let Ok(secret) = fs::read_to_string(paths.root.join(name)) {
            println!("  {label} {}   (needed from other machines)", secret.trim());
        }
    }
    println!("  data     {}", s.data_dir);
    Ok(())
}

pub fn open(paths: &Paths) -> Result<()> {
    let state = if daemon::is_running(paths) {
        running_state(paths)?
    } else {
        start(paths)?
    };
    if open::url(&state.api_url) {
        println!("Opened {}", state.api_url);
    } else {
        println!("Open {} in your browser.", state.api_url);
    }
    Ok(())
}

pub fn logs(paths: &Paths, lines: usize, follow: bool) -> Result<()> {
    let Some(file) = newest_log(paths) else {
        println!("No logs yet in {}", paths.logs.display());
        return Ok(());
    };
    print!("{}", tail(paths, lines));
    if !follow {
        return Ok(());
    }
    let mut pos = fs::metadata(&file)?.len();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let len = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        if len < pos {
            pos = 0; // rotated or truncated
        }
        if len > pos {
            let mut f = fs::File::open(&file)?;
            f.seek(SeekFrom::Start(pos))?;
            let mut new = String::new();
            f.read_to_string(&mut new)?;
            print!("{new}");
            pos = len;
        }
    }
}

fn running_state(paths: &Paths) -> Result<State> {
    State::read(paths).context("shiftWire is running but its state.json is missing or unreadable")
}

fn print_urls(state: &State) {
    println!("  UI       {}", state.api_url);
    println!(
        "  MCP      {}/mcp?agent=NAME   (or: shiftwire mcp --agent NAME)",
        state.api_url
    );
}

/// The daemon logs to `logs/shiftwire.log.<date>`; return the most recent file.
fn newest_log(paths: &Paths) -> Option<PathBuf> {
    fs::read_dir(&paths.logs)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("shiftwire.log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn tail(paths: &Paths, n: usize) -> String {
    let Some(text) = newest_log(paths).and_then(|f| fs::read_to_string(f).ok()) else {
        return "(no log file)\n".into();
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines[lines.len().saturating_sub(n)..].join("\n");
    out.push('\n');
    out
}

fn human_duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::human_duration;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(192), "3m 12s");
        assert_eq!(human_duration(7260), "2h 1m");
    }
}
