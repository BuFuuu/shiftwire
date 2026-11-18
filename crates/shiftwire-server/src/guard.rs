//! Request guard for the control plane.
//!
//! Local requests pass: the `Host` must be loopback (defeats DNS rebinding) and
//! a browser `Origin`, if present, must be loopback too (defeats cross-site
//! requests from pages you happen to visit).
//!
//! When shiftWire listens on another interface, remote clients need the API token
//! (`shiftwire status` shows it): as `Authorization: Bearer …` (agents, scripts), or
//! once as `?token=…` in the browser, which sets an HttpOnly, SameSite=Strict
//! cookie. Writes with the cookie must come from the same origin (no CSRF).

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::server::Ctx;

const COOKIE_NAME: &str = "shiftwire_token";

pub async fn guard(State(c): State<Ctx>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = header(headers, HOST).unwrap_or_default().to_string();
    let origin = header(headers, ORIGIN).map(str::to_string);
    let origin_authority = origin
        .as_deref()
        .and_then(|o| o.split_once("://"))
        .map(|(_, a)| a.to_string());
    let local = is_loopback_authority(&host)
        && origin_authority
            .as_deref()
            .is_none_or(is_loopback_authority);
    if local {
        return next.run(req).await;
    }
    let Some(token) = c.api_token.as_deref() else {
        return (
            StatusCode::FORBIDDEN,
            "shiftWire only accepts requests from this machine",
        )
            .into_response();
    };

    // `?token=…` logs this browser in: set the cookie, then drop the token from the URL.
    if let Some(given) = query_token(req.uri().query()) {
        if !constant_time_eq(given.as_bytes(), token.as_bytes()) {
            return unauthorized();
        }
        let cookie = format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/");
        return (
            StatusCode::SEE_OTHER,
            [
                (LOCATION, req.uri().path().to_string()),
                (SET_COOKIE, cookie),
            ],
        )
            .into_response();
    }
    if bearer_ok(headers, token) {
        return next.run(req).await;
    }
    if cookie_ok(headers, token) {
        let write = !matches!(*req.method(), Method::GET | Method::HEAD);
        let same_origin = origin_authority
            .as_deref()
            .is_none_or(|a| a.eq_ignore_ascii_case(&host));
        if write && !same_origin {
            return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
        }
        return next.run(req).await;
    }
    unauthorized()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "text/html; charset=utf-8")],
        "<!doctype html><title>shiftWire</title><body style=\"font-family:system-ui;padding:3rem\">\
         <h1>shiftWire needs its access token</h1>\
         <p>shiftWire listens beyond this machine, so other addresses need the token once. \
         <code>shiftwire status</code> (or Settings &gt; Network, opened via 127.0.0.1) shows it.</p>\
         <form action=\"/\" method=\"get\"><input name=\"token\" placeholder=\"token\" size=\"52\" autofocus \
         style=\"font:14px monospace;padding:6px 8px\"> <button style=\"padding:6px 14px\">Open</button></form>",
    )
        .into_response()
}

fn query_token(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|p| p.strip_prefix("token="))
}

fn cookie_ok(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| {
            c.trim()
                .strip_prefix(&format!("{COOKIE_NAME}="))
                .map(str::to_string)
        })
        .any(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

/// `127.0.0.1:7777`, `localhost`, `[::1]:7777`, ...
pub fn is_loopback_authority(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

pub fn bearer_ok(headers: &HeaderMap, token: &str) -> bool {
    header(headers, AUTHORIZATION)
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

fn header(headers: &HeaderMap, name: axum::http::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_authorities() {
        for ok in [
            "127.0.0.1:7777",
            "localhost",
            "LOCALHOST:5173",
            "[::1]:7777",
            "127.0.0.1",
        ] {
            assert!(is_loopback_authority(ok), "{ok}");
        }
        for bad in [
            "evil.com",
            "127.0.0.1.evil.com:80",
            "192.168.1.5:7777",
            "[::2]:1",
            "",
        ] {
            assert!(!is_loopback_authority(bad), "{bad}");
        }
    }

    #[test]
    fn bearer_must_match_exactly() {
        let mut h = HeaderMap::new();
        assert!(!bearer_ok(&h, "secret"));
        h.insert(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(bearer_ok(&h, "secret"));
        assert!(!bearer_ok(&h, "secret2"));
    }
}
