//! Open the UI in a browser, preferring a chromeless app window.

use std::process::{Command, Stdio};

/// Returns true if some opener could be launched.
pub fn url(url: &str) -> bool {
    candidates(url).into_iter().any(|(prog, args)| {
        Command::new(prog)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    })
}

fn candidates(url: &str) -> Vec<(&'static str, Vec<String>)> {
    let app = format!("--app={url}");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if cfg!(windows) {
        vec![
            ("cmd", s(&["/C", "start", "", "msedge", &app])),
            ("cmd", s(&["/C", "start", "", url])),
        ]
    } else if cfg!(target_os = "macos") {
        vec![("open", s(&[url]))]
    } else if is_wsl() {
        // Open the *Windows* browser; WSL2 forwards localhost to Windows.
        vec![
            ("wslview", s(&[url])),
            ("cmd.exe", s(&["/C", "start", "", url])),
            (
                "/mnt/c/Windows/System32/cmd.exe",
                s(&["/C", "start", "", url]),
            ),
        ]
    } else {
        vec![
            ("google-chrome", s(&[&app])),
            ("chromium", s(&[&app])),
            ("microsoft-edge", s(&[&app])),
            ("xdg-open", s(&[url])),
        ]
    }
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version").is_ok_and(|v| v.to_lowercase().contains("microsoft"))
}
