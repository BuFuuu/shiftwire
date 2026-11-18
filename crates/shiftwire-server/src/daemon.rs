//! Daemon plumbing: data-dir layout, single-instance lock, state file, detaching.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use fd_lock::{RwLock, RwLockWriteGuard};
use serde::{Deserialize, Serialize};

pub struct Paths {
    pub root: PathBuf,
    pub lock: PathBuf,
    pub state: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    /// Default home of project folders: ~/shiftwire-projects, or inside a custom data dir.
    pub projects: PathBuf,
}

impl Paths {
    pub fn resolve(custom: Option<PathBuf>) -> Result<Self> {
        // The background daemon gets the data dir passed explicitly, so compare
        // instead of checking whether one was given.
        let home = home_dir().ok();
        let root = match custom {
            Some(dir) => dir,
            None => home_dir()?.join(".shiftwire"),
        };
        let projects = match home {
            Some(home) if root == home.join(".shiftwire") => home.join("shiftwire-projects"),
            _ => root.join("projects"),
        };
        let logs = root.join("logs");
        fs::create_dir_all(&logs).with_context(|| format!("cannot create {}", logs.display()))?;
        Ok(Self {
            lock: root.join("shiftwire.lock"),
            state: root.join("state.json"),
            config: root.join("config.toml"),
            logs,
            projects,
            root,
        })
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot find the home directory; pass --data-dir")
}

/// Written by the running daemon so CLI commands can find and control it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub pid: u32,
    pub version: String,
    pub api_url: String,
    pub proxy_addr: String,
    pub started_at: u64,
    /// Authorizes `POST /api/shutdown`. Only readable by the local user.
    pub token: String,
}

impl DaemonState {
    pub fn read(paths: &Paths) -> Option<Self> {
        serde_json::from_slice(&fs::read(&paths.state).ok()?).ok()
    }

    pub fn write(&self, paths: &Paths) -> Result<()> {
        let tmp = paths.state.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        restrict_to_owner(&tmp)?;
        fs::rename(&tmp, &paths.state)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::set_permissions(
        path,
        fs::Permissions::from_mode(0o600),
    )?)
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<()> {
    Ok(()) // the user profile directory is already private on Windows
}

/// Held for the daemon's whole lifetime; dropping it releases the lock.
pub struct InstanceLock {
    _guard: RwLockWriteGuard<'static, File>,
}

pub fn acquire(paths: &Paths) -> Result<InstanceLock> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock)?;
    // Leaked on purpose: one tiny allocation per daemon, gives a 'static guard.
    let lock: &'static mut RwLock<File> = Box::leak(Box::new(RwLock::new(file)));
    let guard = lock.try_write().map_err(|_| {
        anyhow!(
            "shiftWire is already running (data dir {})",
            paths.root.display()
        )
    })?;
    Ok(InstanceLock { _guard: guard })
}

/// True while a daemon holds the lock for this data dir.
pub fn is_running(paths: &Paths) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&paths.lock) else {
        return false;
    };
    RwLock::new(file).try_write().is_err()
}

/// Re-runs this binary as `shiftwire run --detached`, fully detached from the terminal.
pub fn spawn_detached(paths: &Paths) -> Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--data-dir")
        .arg(&paths.root)
        .args(["run", "--detached"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // not in the terminal's job control, survives closing it
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().context("cannot start the background process")?;
    Ok(())
}

/// Last resort when the daemon does not answer `POST /api/shutdown`.
pub fn kill(pid: u32) -> Result<()> {
    let pid = pid.to_string();
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid, "/F"])
            .status()?
    } else {
        Command::new("kill").args(["-TERM", &pid]).status()?
    };
    anyhow::ensure!(status.success(), "could not kill pid {pid}");
    Ok(())
}

/// A secret kept in the data dir (owner-only), created on first use.
pub fn secret(paths: &Paths, name: &str) -> Result<String> {
    let path = paths.root.join(name);
    if let Ok(existing) = fs::read_to_string(&path)
        && !existing.trim().is_empty()
    {
        return Ok(existing.trim().to_string());
    }
    let token = new_token();
    fs::write(&path, &token)?;
    restrict_to_owner(&path)?;
    Ok(token)
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("OS random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Polls `done` every 100 ms until it returns true or `timeout` passes.
pub fn wait_until(timeout: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("shiftwire-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Paths::resolve(Some(dir)).unwrap()
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let paths = temp_paths("lock");
        assert!(!is_running(&paths));
        let held = acquire(&paths).unwrap();
        assert!(is_running(&paths));
        assert!(acquire(&paths).is_err());
        drop(held);
        assert!(!is_running(&paths));
        fs::remove_dir_all(&paths.root).unwrap();
    }

    #[test]
    fn state_round_trips() {
        let paths = temp_paths("state");
        let state = DaemonState {
            pid: 42,
            version: "0.1.0".into(),
            api_url: "http://127.0.0.1:7777".into(),
            proxy_addr: "127.0.0.1:8080".into(),
            started_at: 1,
            token: new_token(),
        };
        state.write(&paths).unwrap();
        let back = DaemonState::read(&paths).unwrap();
        assert_eq!(back.pid, 42);
        assert_eq!(back.token.len(), 48);
        fs::remove_dir_all(&paths.root).unwrap();
    }
}
