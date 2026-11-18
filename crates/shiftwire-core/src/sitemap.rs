//! The site map: every endpoint seen, per host, with what happened there.
//!
//! IDs in paths are collapsed (`/users/7` and `/users/8` are `/users/{id}`), so
//! a crawl of 20,000 users is one endpoint, not 20,000.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Endpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Path pattern, e.g. `/api/users/{id}`.
    pub path: String,
    pub methods: Vec<String>,
    pub count: u64,
    /// Responses by class: 2xx, 3xx, 4xx, 5xx, and no response at all.
    pub ok: u64,
    pub redirect: u64,
    pub client_error: u64,
    pub server_error: u64,
    pub failed: u64,
    /// Requests sent by AI agents.
    pub by_agents: u64,
    /// At least one request here was marked as important.
    pub marked: bool,
    pub last_seen: u64,
    /// The host is in the project's scope (Settings).
    pub in_scope: bool,
    /// Responses with status 404.
    pub not_found: u64,
    /// It exists: at least one response other than 404 (or no response at all).
    pub found: bool,
}

/// One row of `SELECT ... GROUP BY` from the store, before merging patterns.
pub(crate) struct Group {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub method: String,
    pub count: u64,
    pub ok: u64,
    pub redirect: u64,
    pub client_error: u64,
    pub server_error: u64,
    pub failed: u64,
    pub not_found: u64,
    pub by_agents: u64,
    pub marked: bool,
    pub last_seen: u64,
}

/// host, port, scheme, path pattern: one site map entry.
type Key = (String, u16, String, String);

pub(crate) fn build(groups: Vec<Group>) -> Vec<Endpoint> {
    let mut merged: BTreeMap<Key, (Endpoint, BTreeSet<String>)> = BTreeMap::new();
    for g in groups {
        let path = pattern(&g.path);
        let key = (g.host.clone(), g.port, g.scheme.clone(), path.clone());
        let (e, methods) = merged.entry(key).or_insert_with(|| {
            let e = Endpoint {
                scheme: g.scheme.clone(),
                host: g.host.clone(),
                port: g.port,
                path,
                ..Endpoint::default()
            };
            (e, BTreeSet::new())
        });
        methods.insert(g.method);
        e.count += g.count;
        e.ok += g.ok;
        e.redirect += g.redirect;
        e.client_error += g.client_error;
        e.server_error += g.server_error;
        e.failed += g.failed;
        e.not_found += g.not_found;
        e.by_agents += g.by_agents;
        e.marked |= g.marked;
        e.last_seen = e.last_seen.max(g.last_seen);
    }
    merged
        .into_values()
        .map(|(mut e, methods)| {
            e.methods = methods.into_iter().collect();
            e.found = e.count > e.not_found + e.failed;
            e
        })
        .collect()
}

/// `/api/users/7/orders/3f2a…` -> `/api/users/{id}/orders/{hash}`.
pub fn pattern(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
                "{id}"
            } else if is_uuid(seg) {
                "{uuid}"
            } else if seg.len() >= 16 && seg.bytes().all(|b| b.is_ascii_hexdigit()) {
                "{hash}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_collapse_into_patterns() {
        assert_eq!(pattern("/api/users/7"), "/api/users/{id}");
        assert_eq!(
            pattern("/a/550e8400-e29b-41d4-a716-446655440000/b"),
            "/a/{uuid}/b"
        );
        assert_eq!(pattern("/files/3f2a9c0b1d4e5f60aa"), "/files/{hash}");
        assert_eq!(
            pattern("/v2/login/"),
            "/v2/login/",
            "short or non-numeric segments stay"
        );
        assert_eq!(pattern("/"), "/");
    }

    #[test]
    fn groups_merge_into_endpoints() {
        let g = |path: &str, method: &str, status_ok: u64, agents: u64, marked: bool| Group {
            scheme: "https".into(),
            host: "shop.test".into(),
            port: 443,
            path: path.into(),
            method: method.into(),
            count: 1,
            ok: status_ok,
            redirect: 0,
            client_error: 1 - status_ok,
            server_error: 0,
            failed: 0,
            not_found: 1 - status_ok,
            by_agents: agents,
            marked,
            last_seen: 5,
        };
        let eps = build(vec![
            g("/api/users/7", "GET", 1, 0, false),
            g("/api/users/8", "GET", 0, 1, true),
            g("/api/users/8", "DELETE", 1, 1, false),
            g("/login", "POST", 1, 0, false),
        ]);
        assert_eq!(eps.len(), 2);
        let users = eps.iter().find(|e| e.path == "/api/users/{id}").unwrap();
        assert_eq!(
            (users.count, users.ok, users.client_error, users.by_agents),
            (3, 2, 1, 2)
        );
        assert_eq!(users.methods, ["DELETE", "GET"]);
        assert!(users.marked);
        assert!(users.found, "one 404 among real responses: it exists");

        let missing = build(vec![
            g("/admin", "GET", 0, 0, false),
            g("/admin", "HEAD", 0, 0, false),
        ]);
        assert!(!missing[0].found, "only 404s: not found");
    }
}
