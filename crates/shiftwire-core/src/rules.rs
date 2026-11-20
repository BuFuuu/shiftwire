//! Match & replace / alert rules, applied to every request passing the proxy
//! before it is forwarded. Rules belong to the project (like the scope).
//!
//! * **Replace**: rewrite what matches, e.g. `^User-Agent: .*$` → `User-Agent: mine`.
//! * **Alert**: change nothing, but tag the request and tell the operator when the
//!   pattern matches, or (with `when_missing`) when it does not, e.g. "User-Agent
//!   is not mine": `^User-Agent: mine$` + when missing. With `block`, the request is
//!   not sent at all (shiftWire answers 403). Agents' own MCP sends obey alert rules
//!   too ([`Rules::check`]), so an agent cannot get around a block.
//!
//! Header rules see one `Name: value` line per header (`^`/`$` work per line);
//! body rules see uncompressed bodies with a known length up to the capture limit.

use hyper::HeaderMap;
use hyper::header::{HeaderName, HeaderValue};
use regex::Regex;
use regex::bytes::Regex as BytesRegex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub mode: RuleMode,
    pub part: RulePart,
    /// A regular expression (Rust syntax).
    pub pattern: String,
    /// Replace mode: the new text; `$1` refers to a capture group.
    #[serde(default)]
    pub replace: String,
    /// Alert mode: alert when the pattern does *not* match.
    #[serde(default)]
    pub when_missing: bool,
    /// Alert mode: also stop the request (shiftWire answers 403 instead of sending it).
    #[serde(default)]
    pub block: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleMode {
    Replace,
    Alert,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulePart {
    Headers,
    Body,
}

/// What the rules did to one request.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// `replaced:<rule>` and `alert:<rule>`, stored as tags on the flow.
    pub tags: Vec<String>,
    /// Names of the alert rules that fired.
    pub alerts: Vec<String>,
    /// The first alert rule with `block` that fired: the request must not be sent.
    pub blocked: Option<String>,
}

/// Rules with their patterns compiled, ready for the proxy.
#[derive(Default)]
pub struct Rules(Vec<(Rule, Regex, BytesRegex)>);

impl Rules {
    /// Checks and compiles; the error names the rule with a bad pattern.
    pub fn compile(rules: &[Rule]) -> Result<Self, String> {
        let mut out = Vec::new();
        for rule in rules {
            if rule.name.trim().is_empty() {
                return Err("every rule needs a name".into());
            }
            let pattern = format!("(?m){}", rule.pattern);
            let bad = |e: regex::Error| format!("rule \"{}\": {e}", rule.name);
            out.push((
                rule.clone(),
                Regex::new(&pattern).map_err(bad)?,
                BytesRegex::new(&pattern).map_err(bad)?,
            ));
        }
        Ok(Self(out))
    }

    fn active(&self, part: RulePart) -> impl Iterator<Item = &(Rule, Regex, BytesRegex)> {
        self.0
            .iter()
            .filter(move |(r, ..)| r.enabled && r.part == part)
    }

    /// Whether any enabled rule looks at bodies (only then is a body read first).
    pub fn wants_body(&self) -> bool {
        self.active(RulePart::Body).next().is_some()
    }

    pub fn apply_headers(&self, headers: &mut HeaderMap, out: &mut Outcome) {
        if self.active(RulePart::Headers).next().is_none() {
            return;
        }
        let original = headers
            .iter()
            .map(|(n, v)| {
                format!(
                    "{}: {}",
                    title_case(n.as_str()),
                    String::from_utf8_lossy(v.as_bytes())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut text = original.clone();
        for (rule, re, _) in self.active(RulePart::Headers) {
            if rule.mode == RuleMode::Replace && re.is_match(&text) {
                text = re.replace_all(&text, rule.replace.as_str()).into_owned();
                out.tags.push(tag("replaced", &rule.name));
            }
        }
        self.alert_headers(&text, out);
        if text != original {
            *headers = parse_headers(&text);
        }
    }

    /// Returns whether the body changed (the caller fixes Content-Length).
    pub fn apply_body(&self, body: &mut Vec<u8>, out: &mut Outcome) -> bool {
        let mut changed = false;
        for (rule, _, re) in self.active(RulePart::Body) {
            if rule.mode == RuleMode::Replace && re.is_match(body) {
                *body = re.replace_all(body, rule.replace.as_bytes()).into_owned();
                out.tags.push(tag("replaced", &rule.name));
                changed = true;
            }
        }
        self.alert_body(body, out);
        changed
    }

    /// Alert rules only (nothing is changed): for requests shiftWire sends itself on an
    /// agent's behalf. `headers` is one `Name: value` line per header.
    pub fn check(&self, headers: &str, body: &[u8], out: &mut Outcome) {
        self.alert_headers(headers, out);
        self.alert_body(body, out);
    }

    fn alert_headers(&self, text: &str, out: &mut Outcome) {
        for (rule, re, _) in self.active(RulePart::Headers) {
            if rule.mode == RuleMode::Alert && re.is_match(text) != rule.when_missing {
                out.fire(rule);
            }
        }
    }

    fn alert_body(&self, body: &[u8], out: &mut Outcome) {
        for (rule, _, re) in self.active(RulePart::Body) {
            if rule.mode == RuleMode::Alert && re.is_match(body) != rule.when_missing {
                out.fire(rule);
            }
        }
    }
}

impl Outcome {
    fn fire(&mut self, rule: &Rule) {
        self.tags.push(tag("alert", &rule.name));
        self.alerts.push(rule.name.clone());
        if rule.block && self.blocked.is_none() {
            self.blocked = Some(rule.name.clone());
        }
    }

    /// Why the request was stopped, for the flow's error and the client.
    pub fn block_reason(&self) -> Option<String> {
        self.blocked.as_ref().map(|rule| {
            format!("blocked by the alert rule \"{rule}\" (Settings → Match & replace)")
        })
    }
}

/// `user-agent` -> `User-Agent`, the way headers are usually written (and patterns too).
fn title_case(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut c = part.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// `alert:UA check` -> `alert:ua-check` (tags have no spaces).
fn tag(kind: &str, name: &str) -> String {
    let name: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .take(48)
        .collect();
    format!("{kind}:{name}")
}

/// Lines that are not valid `Name: value` headers after a replacement are dropped,
/// so a rule can also remove a header (replace `^X-Debug: .*$` with nothing).
fn parse_headers(text: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.trim().as_bytes()),
            HeaderValue::from_str(value.trim()),
        ) {
            headers.append(name, value);
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(mode: RuleMode, part: RulePart, pattern: &str, replace: &str) -> Rule {
        Rule {
            id: String::new(),
            name: "UA check".into(),
            enabled: true,
            mode,
            part,
            pattern: pattern.into(),
            replace: replace.into(),
            when_missing: false,
            block: false,
        }
    }

    fn headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("user-agent", HeaderValue::from_static("curl/8.0"));
        h.insert("x-debug", HeaderValue::from_static("1"));
        h
    }

    #[test]
    fn replaces_and_removes_headers() {
        let rules = Rules::compile(&[
            rule(
                RuleMode::Replace,
                RulePart::Headers,
                "^User-Agent: .*$",
                "User-Agent: Mine/1.0",
            ),
            rule(RuleMode::Replace, RulePart::Headers, "^X-Debug: .*$", ""),
        ])
        .unwrap();
        let (mut h, mut out) = (headers(), Outcome::default());
        rules.apply_headers(&mut h, &mut out);
        assert_eq!(h["user-agent"], "Mine/1.0");
        assert!(!h.contains_key("x-debug"));
        assert_eq!(out.tags, ["replaced:ua-check", "replaced:ua-check"]);
        assert!(out.alerts.is_empty());
    }

    #[test]
    fn alerts_when_matching_or_when_missing_and_change_nothing() {
        let mut missing = rule(
            RuleMode::Alert,
            RulePart::Headers,
            "^User-Agent: Mine/1.0$",
            "",
        );
        missing.when_missing = true;
        let rules = Rules::compile(&[missing]).unwrap();
        let (mut h, mut out) = (headers(), Outcome::default());
        rules.apply_headers(&mut h, &mut out);
        assert_eq!(h, headers(), "alerts never modify the request");
        assert_eq!(out.alerts, ["UA check"]);

        let matching = rule(RuleMode::Alert, RulePart::Body, "password=", "");
        let rules = Rules::compile(&[matching]).unwrap();
        let mut out = Outcome::default();
        assert!(!rules.apply_body(&mut b"user=a&password=b".to_vec(), &mut out));
        assert_eq!(out.tags, ["alert:ua-check"]);
    }

    #[test]
    fn blocking_alerts_and_checks_without_changes() {
        let mut no_pw = rule(RuleMode::Alert, RulePart::Body, "password=", "");
        no_pw.block = true;
        let ua = rule(RuleMode::Alert, RulePart::Headers, "^User-Agent: curl", "");
        let rules = Rules::compile(&[ua, no_pw]).unwrap();
        let mut out = Outcome::default();
        rules.check("Host: a.test\nUser-Agent: curl/8", b"password=x", &mut out);
        assert_eq!(out.alerts.len(), 2);
        assert_eq!(
            out.blocked.as_deref(),
            Some("UA check"),
            "only the blocking rule"
        );
        assert!(
            out.block_reason()
                .unwrap()
                .contains("blocked by the alert rule")
        );
        let mut quiet = Outcome::default();
        rules.check("User-Agent: mine", b"user=a", &mut quiet);
        assert!(quiet.alerts.is_empty() && quiet.blocked.is_none());
    }

    #[test]
    fn body_replacement_and_disabled_rules() {
        let mut off = rule(RuleMode::Replace, RulePart::Body, "admin=0", "admin=1");
        let rules = Rules::compile(std::slice::from_ref(&off)).unwrap();
        assert!(rules.wants_body());
        let mut body = b"id=7&admin=0".to_vec();
        assert!(rules.apply_body(&mut body, &mut Outcome::default()));
        assert_eq!(body, b"id=7&admin=1");

        off.enabled = false;
        let rules = Rules::compile(&[off]).unwrap();
        assert!(!rules.wants_body());
    }

    #[test]
    fn bad_patterns_are_reported_with_the_rule_name() {
        let err = Rules::compile(&[rule(RuleMode::Alert, RulePart::Headers, "(", "")])
            .err()
            .unwrap();
        assert!(err.contains("UA check"), "{err}");
    }
}
