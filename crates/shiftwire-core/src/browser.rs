//! The AI browser: a Chromium-family browser that shiftWire starts. It talks only to its
//! own proxy listener (so History marks its requests `browser`), trusts shiftWire's CA
//! without installing it, and keeps cookies and logins in the project folder.
//! The operator opens and closes it; agents steer it through shiftWire's MCP, which
//! hands their browser tools to Playwright MCP connected to the DevTools (CDP) port.

use std::path::{Path, PathBuf};

/// Chromium-family browsers, most common first: names on `PATH`, then usual install paths.
const NAMES: [&str; 8] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "chrome",
    "msedge",
];
const PATHS: [&str; 5] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
];

/// The configured browser, or the first one found.
pub(crate) fn find(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured {
        return path.exists().then(|| path.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let on_path = NAMES.iter().find_map(|name| {
        dirs.iter().find_map(|dir| {
            [dir.join(name), dir.join(format!("{name}.exe"))]
                .into_iter()
                .find(|p| p.is_file())
        })
    });
    on_path.or_else(|| PATHS.iter().map(PathBuf::from).find(|p| p.is_file()))
}

/// Command-line flags: our proxy for everything (loopback too), our CA's key trusted,
/// a profile of its own, no first-run dialogs, and none of the browser's own background
/// traffic (updates, dictionaries, sync), so History shows what the pages do.
/// The DevTools port (for Playwright) listens on loopback only. Opens shiftWire's welcome page.
pub(crate) fn args(
    proxy_port: u16,
    cdp_port: u16,
    profile: &Path,
    spki_hashes: &str,
) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile.display()),
        format!("--remote-debugging-port={cdp_port}"),
        "--remote-debugging-address=127.0.0.1".into(),
        format!("--proxy-server=http://127.0.0.1:{proxy_port}"),
        "--proxy-bypass-list=<-loopback>".into(),
        format!("--ignore-certificate-errors-spki-list={spki_hashes}"),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--password-store=basic".into(),
        // Chromium warns about the certificate flag above in an info bar on every start;
        // automation tools pass this to switch that warning off.
        "--test-type".into(),
        "--disable-background-networking".into(),
        "--disable-component-update".into(),
        "--disable-sync".into(),
        "--disable-default-apps".into(),
        "--disable-features=OptimizationGuideModelDownloading,OptimizationHints,OptimizationHintsFetching,MediaRouter,Translate,ChromeSignin".into(),
        "http://shiftwire/".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_route_everything_through_the_browser_listener() {
        let args = args(41234, 9333, Path::new("/p/browser"), "AAA=,BBB=");
        assert!(args.contains(&"--remote-debugging-port=9333".to_string()));
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:41234".to_string()));
        assert!(args.contains(&"--user-data-dir=/p/browser".to_string()));
        assert!(args.contains(&"--ignore-certificate-errors-spki-list=AAA=,BBB=".to_string()));
        assert!(args.contains(&"--proxy-bypass-list=<-loopback>".to_string()));
    }

    #[test]
    fn a_configured_path_must_exist() {
        assert_eq!(find(Some(Path::new("/nonexistent/chrome"))), None);
        let me = std::env::current_exe().unwrap();
        assert_eq!(find(Some(&me)), Some(me.clone()));
    }
}
