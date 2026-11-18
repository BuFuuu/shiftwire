//! Which hosts AI agents may touch.
//!
//! The human sets the scope; agents are held to it. With no include rules,
//! nothing is in scope: an agent cannot send anything until the human says where.
//! Human traffic (browser, Repeater) is never restricted.

use std::io;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scope {
    /// `shop.test` (exact) or `*.shop.test` (the domain and all subdomains).
    pub include: Vec<String>,
    /// Same patterns; exclusions win over inclusions.
    pub exclude: Vec<String>,
}

impl Scope {
    /// Trims, lowercases and de-duplicates patterns; `https://a.test/x` becomes `a.test`.
    pub fn normalized(self) -> io::Result<Self> {
        Ok(Self {
            include: clean(self.include)?,
            exclude: clean(self.exclude)?,
        })
    }

    pub fn allows(&self, host: &str) -> bool {
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        !self.exclude.iter().any(|p| matches(p, &host))
            && self.include.iter().any(|p| matches(p, &host))
    }

    /// One line for messages: `in: a.test, *.b.test · out: admin.b.test`.
    pub fn describe(&self) -> String {
        if self.include.is_empty() {
            return "empty (nothing is in scope yet)".into();
        }
        let mut out = format!("in: {}", self.include.join(", "));
        if !self.exclude.is_empty() {
            out.push_str(&format!(" · out: {}", self.exclude.join(", ")));
        }
        out
    }
}

fn matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
        None => host == pattern,
    }
}

fn clean(patterns: Vec<String>) -> io::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in patterns {
        let mut p = raw.trim().to_ascii_lowercase();
        if let Some((_, rest)) = p.split_once("://") {
            p = rest.to_string();
        }
        p = p
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_string();
        // Scope is per host, not per port: drop ports (and IPv6 brackets).
        if let Some(rest) = p.strip_prefix('[') {
            p = rest.split(']').next().unwrap_or_default().to_string();
        } else if let Some((host, port)) = p.rsplit_once(':')
            && port.chars().all(|c| c.is_ascii_digit())
            && !host.contains(':')
        {
            p = host.to_string();
        }
        if p.is_empty() {
            continue;
        }
        let body = p.strip_prefix("*.").unwrap_or(&p);
        if body.is_empty() || body.contains(['*', ' ', '@']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid scope pattern {raw:?}: use host or *.domain"),
            ));
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(include: &[&str], exclude: &[&str]) -> Scope {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect();
        Scope {
            include: v(include),
            exclude: v(exclude),
        }
        .normalized()
        .unwrap()
    }

    #[test]
    fn empty_scope_allows_nothing() {
        assert!(!Scope::default().allows("anything.test"));
    }

    #[test]
    fn includes_wildcards_and_exclusions() {
        let s = scope(&["shop.test", "*.api.test"], &["admin.api.test"]);
        assert!(s.allows("SHOP.test"));
        assert!(!s.allows("www.shop.test"), "exact patterns are exact");
        assert!(s.allows("api.test") && s.allows("v2.api.test"));
        assert!(!s.allows("admin.api.test"), "exclusions win");
        assert!(!s.allows("evilapi.test"));
    }

    #[test]
    fn patterns_are_cleaned_up() {
        let s = scope(&[" HTTPS://Shop.Test:8443/login ", "shop.test", ""], &[]);
        assert_eq!(s.include, vec!["shop.test"]);
        assert!(
            Scope {
                include: vec!["a b".into()],
                exclude: vec![]
            }
            .normalized()
            .is_err()
        );
        assert!(
            Scope {
                include: vec!["*.".into()],
                exclude: vec![]
            }
            .normalized()
            .is_err()
        );
        let v6 = scope(&["[::1]:8080"], &[]);
        assert_eq!(v6.include, vec!["::1"]);
        assert!(v6.allows("[::1]") && v6.allows("::1"));
    }
}
