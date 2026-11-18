//! Minimal HTTP client the CLI uses to talk to the running daemon.

use std::time::Duration;

use anyhow::Result;
use shiftwire_core::Status;

const TIMEOUT: Duration = Duration::from_secs(2);

pub fn health(api_url: &str) -> bool {
    ureq::get(&format!("{api_url}/api/health"))
        .timeout(TIMEOUT)
        .call()
        .is_ok()
}

pub fn status(api_url: &str) -> Result<Status> {
    let resp = ureq::get(&format!("{api_url}/api/status"))
        .timeout(TIMEOUT)
        .call()?;
    Ok(resp.into_json()?)
}

pub fn shutdown(api_url: &str, token: &str) -> Result<()> {
    ureq::post(&format!("{api_url}/api/shutdown"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(TIMEOUT)
        .call()?;
    Ok(())
}
