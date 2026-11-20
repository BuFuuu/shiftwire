//! Persistent settings (`config.toml` in the data dir).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// UI, REST API and WebSocket. Loopback unless explicitly changed.
    pub api_addr: SocketAddr,
    /// MCP for AI agents (`/mcp`), the only way in for agents besides the proxy.
    pub mcp_addr: SocketAddr,
    /// The intercepting proxy clients (browser, VM, AI tools) point at.
    pub proxy_addr: SocketAddr,
    /// Where new projects are created (one folder each). Default: ~/shiftwire-projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects_dir: Option<PathBuf>,
    /// Bodies larger than this are stored truncated (forwarding is never affected).
    pub max_body_bytes: usize,
    /// Hosts tunneled without interception: exact names or `*.example.com`.
    /// Defaults to AI vendors' APIs, so agents' own traffic is not captured.
    pub passthrough: Vec<String>,
    /// Verify target servers' certificates. Off by default: staging targets often have bad ones.
    pub verify_upstream_tls: bool,
    /// The AI browser (Chrome, Edge or Chromium). Default: the first one found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_addr: ([127, 0, 0, 1], 7777).into(),
            mcp_addr: ([127, 0, 0, 1], 7778).into(),
            proxy_addr: ([127, 0, 0, 1], 8080).into(),
            projects_dir: None,
            max_body_bytes: 2 * 1024 * 1024,
            // An agent's own model API traffic (long-lived streams carrying an API key)
            // is tunneled untouched: never decrypted, never stored.
            passthrough: ["*.anthropic.com", "*.claude.ai", "*.openai.com"]
                .map(String::from)
                .to_vec(),
            verify_upstream_tls: false,
            browser_path: None,
        }
    }
}

impl Config {
    /// Whether `host` is on the passthrough list (case-insensitive).
    pub fn is_passthrough(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.passthrough.iter().any(|pattern| {
            let pattern = pattern.trim().to_ascii_lowercase();
            match pattern.strip_prefix("*.") {
                Some(suffix) => host.ends_with(&format!(".{suffix}")),
                None => host == pattern,
            }
        })
    }

    /// Reads `path`, or writes the defaults there on first run.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = Self::default();
                let text = toml::to_string_pretty(&cfg).map_err(io::Error::other)?;
                fs::write(path, text)?;
                Ok(cfg)
            }
            Err(e) => Err(e),
        }
    }

    /// Writes the settings (atomically: a crash never leaves half a file).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reloads_defaults() {
        let dir = std::env::temp_dir().join(format!("shiftwire-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let _ = fs::remove_file(&path);

        let first = Config::load_or_create(&path).unwrap();
        assert!(path.exists());
        let again = Config::load_or_create(&path).unwrap();
        assert_eq!(first.api_addr, again.api_addr);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn passthrough_patterns() {
        let cfg = Config {
            passthrough: vec!["pinned.app".into(), "*.apple.com".into()],
            ..Config::default()
        };
        assert!(cfg.is_passthrough("PINNED.app"));
        assert!(cfg.is_passthrough("api.apple.com"));
        assert!(!cfg.is_passthrough("apple.com"));
        assert!(!cfg.is_passthrough("evilapple.com"));
        assert!(
            Config::default().is_passthrough("api.anthropic.com"),
            "agents' own API traffic is left alone"
        );
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("projects_dir = \"/tmp/p\"").unwrap();
        assert_eq!(cfg.projects_dir, Some(PathBuf::from("/tmp/p")));
        assert_eq!(cfg.proxy_addr.port(), 8080);
        assert_eq!(cfg.mcp_addr.port(), 7778);
    }
}
