//! Masking: agents see stand-in names for the target, so its real names are not sent
//! to the AI provider. A site (`example.com`) gets an alias (`target1.masked`) that
//! also covers its subdomains (`api.example.com` ↔ `api.target1.masked`). What agents
//! send over MCP is translated back to the real names; what they get is masked.
//! `.masked` never resolves, so an alias only works through shiftWire's MCP.
//!
//! This is the whole feature in the core: the MCP adapter calls [`Masker::inbound`]
//! and [`Masker::outbound`] in one place, and the proxy refuses agents' own proxy use.

use std::io;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How far masking reaches in what agents get back (each level includes the one before).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskLevel {
    #[default]
    Off,
    /// shiftWire's own words fully; in target content only addresses: URLs, address
    /// headers (`Host`, `Origin`, `Referer`, `Location`, cookies) and e-mail addresses.
    Addresses,
    /// Every occurrence of a masked name, anywhere. Images are withheld.
    Content,
    /// Also the operator's words (`[word1]`), an alias for every other host agents
    /// could see (`h1.masked`, except the allow-list), and encoded names withheld.
    Strict,
}

/// One masked site: the real registrable name and what agents see instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskSite {
    pub real: String,
    /// `<label>.masked`; empty means "pick the next `targetN.masked`".
    #[serde(default)]
    pub alias: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Masking {
    pub level: MaskLevel,
    #[serde(default)]
    pub sites: Vec<MaskSite>,
    /// Strict: company, product and people names → `[word1]`, `[word2]`, ...
    #[serde(default)]
    pub words: Vec<String>,
    /// Strict: hosts agents may see as they are (`cdnjs.cloudflare.com`, `*.gstatic.com`).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Strict: the aliases given to other hosts so far (`h1.masked`), kept per project.
    #[serde(default)]
    pub auto: Vec<MaskSite>,
}

fn host_pattern() -> Regex {
    Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$").expect("valid")
}

impl Masking {
    /// Lowercased, checked, aliases filled in. Names may not overlap.
    pub fn normalized(mut self) -> io::Result<Self> {
        let bad = |m: String| io::Error::new(io::ErrorKind::InvalidInput, m);
        let host = host_pattern();
        let label = Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$").expect("valid");
        self.sites.retain(|s| !s.real.trim().is_empty());
        for site in &mut self.sites {
            site.real = site
                .real
                .trim()
                .trim_start_matches("*.")
                .to_ascii_lowercase();
            site.alias = site.alias.trim().to_ascii_lowercase();
            if !host.is_match(&site.real) || site.real.ends_with(".masked") {
                return Err(bad(format!(
                    "{}: a site is a host name like example.com",
                    site.real
                )));
            }
        }
        let mut n = 1;
        for i in 0..self.sites.len() {
            if self.sites[i].alias.is_empty() {
                while self
                    .sites
                    .iter()
                    .any(|s| s.alias == format!("target{n}.masked"))
                {
                    n += 1;
                }
                self.sites[i].alias = format!("target{n}.masked");
            }
            let alias = &self.sites[i].alias;
            if !alias
                .strip_suffix(".masked")
                .is_some_and(|l| label.is_match(l))
                || alias.starts_with('h')
                    && alias[1..]
                        .split('.')
                        .next()
                        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(bad(format!(
                    "{alias}: an alias is one name plus .masked, e.g. target1.masked (h1.masked … are for Strict)"
                )));
            }
        }
        for (i, a) in self.sites.iter().enumerate() {
            for b in &self.sites[i + 1..] {
                let within = |x: &str, y: &str| x == y || x.ends_with(&format!(".{y}"));
                if within(&a.real, &b.real) || within(&b.real, &a.real) || a.alias == b.alias {
                    return Err(bad(format!("{} and {} overlap", a.real, b.real)));
                }
            }
        }
        let mut words: Vec<String> = Vec::new();
        for word in self
            .words
            .iter()
            .map(|w| w.trim())
            .filter(|w| !w.is_empty())
        {
            if word.chars().count() < 3 {
                return Err(bad(format!(
                    "\"{word}\": words need at least 3 characters (shorter ones replace too much)"
                )));
            }
            if !words.iter().any(|w| w.eq_ignore_ascii_case(word)) {
                words.push(word.to_string());
            }
        }
        self.words = words;
        self.allow = self
            .allow
            .iter()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        // Auto aliases: well-formed, and not for hosts a site already covers.
        let sites = self.sites.clone();
        self.auto.retain(|a| {
            host.is_match(&a.real)
                && a.alias.starts_with('h')
                && a.alias.ends_with(".masked")
                && !sites.iter().any(|s| covers(&s.real, &a.real))
        });
        Ok(self)
    }
}

/// `site` is `host` or one of its parents.
fn covers(site: &str, host: &str) -> bool {
    host == site || host.ends_with(&format!(".{site}"))
}

/// A name and its stand-in, with case-insensitive patterns for both.
struct Pair {
    real: String,
    alias: String,
    real_re: Regex,
    alias_re: Regex,
}

impl Pair {
    fn new(real: &str, alias: &str) -> Self {
        let pattern =
            |name: &str| Regex::new(&format!("(?i){}", regex::escape(name))).expect("escaped");
        Self {
            real: real.to_string(),
            alias: alias.to_string(),
            real_re: pattern(real),
            alias_re: pattern(alias),
        }
    }
}

/// Strict: aliases for hosts seen so far, and whether new ones need saving.
#[derive(Default)]
struct Auto {
    pairs: Vec<Pair>,
    dirty: bool,
}

/// Ready-to-use translation for one [`Masking`].
pub struct Masker {
    pub level: MaskLevel,
    sites: Vec<Pair>,
    words: Vec<Pair>,
    allow: Vec<String>,
    auto: Mutex<Auto>,
    /// Places in target content that hold addresses (for [`MaskLevel::Addresses`]).
    addresses: Vec<Regex>,
    /// Hosts in address places (Strict: each gets an alias).
    hosts: Vec<Regex>,
}

impl Masker {
    pub fn new(masking: &Masking) -> Self {
        let off = masking.level == MaskLevel::Off;
        let strict = masking.level == MaskLevel::Strict;
        let regexes = |list: &[&str]| list.iter().map(|p| Regex::new(p).expect("valid")).collect();
        Self {
            level: masking.level,
            sites: if off {
                Vec::new()
            } else {
                masking
                    .sites
                    .iter()
                    .map(|s| Pair::new(&s.real, &s.alias))
                    .collect()
            },
            words: if strict {
                masking
                    .words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| Pair::new(w, &format!("[word{}]", i + 1)))
                    .collect()
            } else {
                Vec::new()
            },
            allow: masking.allow.clone(),
            auto: Mutex::new(Auto {
                pairs: if strict {
                    masking
                        .auto
                        .iter()
                        .map(|a| Pair::new(&a.real, &a.alias))
                        .collect()
                } else {
                    Vec::new()
                },
                dirty: false,
            }),
            addresses: regexes(&[
                // URLs: plain, JSON-escaped and percent-encoded slashes, then host and path.
                r#"(?i)(//|\\/\\/|%2f%2f)[^\s"'<>()\[\]{}|^`]*"#,
                // Headers that carry addresses (whole line: cookies may name domains).
                r"(?im)^(host|origin|referer|location|content-location|access-control-allow-origin|link|refresh|set-cookie|cookie|forwarded|x-forwarded-host|x-original-host)\s*:[^\n]*",
                // E-mail addresses.
                r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+",
            ]),
            hosts: regexes(&[
                r"(?i)(?://|\\/\\/|%2f%2f)(?:[^/@\s]*@)?([a-z0-9][a-z0-9.-]*\.[a-z][a-z0-9-]+)",
                r"(?im)^(?:host|origin|referer|location|x-forwarded-host|x-original-host)\s*:\s*(?:[a-z]+://)?([a-z0-9][a-z0-9.-]*\.[a-z][a-z0-9-]+)",
                r"(?i)@([a-z0-9][a-z0-9.-]*\.[a-z][a-z0-9-]+)",
                r"(?i)domain=\.?([a-z0-9][a-z0-9.-]*\.[a-z][a-z0-9-]+)",
            ]),
        }
    }

    pub fn active(&self) -> bool {
        self.level != MaskLevel::Off
    }

    /// Content and Strict withhold what cannot be masked: screenshots, images, page scripts.
    pub fn hides_images(&self) -> bool {
        self.level >= MaskLevel::Content
    }

    /// Strict: gives `host` an alias unless a site covers it or it is allowed.
    pub fn learn(&self, host: &str) {
        let host = host
            .trim()
            .trim_start_matches("*.")
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let allowed = self.allow.iter().any(|a| match a.strip_prefix("*.") {
            Some(suffix) => covers(suffix, &host),
            None => *a == host,
        });
        if self.level != MaskLevel::Strict
            || allowed
            || host.ends_with(".masked")
            || !host_pattern().is_match(&host)
            || self.sites.iter().any(|s| covers(&s.real, &host))
        {
            return;
        }
        let mut auto = self.auto.lock().expect("auto lock");
        if auto.pairs.iter().any(|p| p.real == host) {
            return;
        }
        let n = auto
            .pairs
            .iter()
            .filter_map(|p| {
                p.alias
                    .strip_prefix('h')?
                    .strip_suffix(".masked")?
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        auto.pairs.push(Pair::new(&host, &format!("h{n}.masked")));
        auto.pairs.sort_by_key(|p| std::cmp::Reverse(p.real.len())); // the most specific first
        auto.dirty = true;
    }

    /// Strict: the aliases given to other hosts, to keep in the project.
    pub fn auto_sites(&self) -> Vec<MaskSite> {
        let auto = self.auto.lock().expect("auto lock");
        auto.pairs
            .iter()
            .map(|p| MaskSite {
                real: p.real.clone(),
                alias: p.alias.clone(),
            })
            .collect()
    }

    /// Whether new aliases were given since the last call (then save [`Self::auto_sites`]).
    pub fn take_new_aliases(&self) -> bool {
        std::mem::take(&mut self.auto.lock().expect("auto lock").dirty)
    }

    /// Real names → stand-ins, everywhere in `text`.
    pub fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        for p in &self.sites {
            out = swap(&out, &p.real_re, &p.alias);
        }
        for p in &self.auto.lock().expect("auto lock").pairs {
            out = swap(&out, &p.real_re, &p.alias);
        }
        for p in &self.words {
            out = swap_word(&out, &p.real_re, &p.alias);
        }
        out
    }

    /// Stand-ins → real names in every string of an agent's arguments.
    pub fn inbound(&self, args: &Value) -> Value {
        match args {
            Value::String(s) => {
                let mut out = s.clone();
                for p in &self.sites {
                    out = swap(&out, &p.alias_re, &p.real);
                }
                for p in &self.auto.lock().expect("auto lock").pairs {
                    out = swap(&out, &p.alias_re, &p.real);
                }
                for p in &self.words {
                    out = p
                        .alias_re
                        .replace_all(&out, regex::NoExpand(&p.real))
                        .into_owned();
                }
                Value::String(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| self.inbound(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.inbound(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// What an agent gets back. `Addresses`: shiftWire's own words fully, and in target
    /// content (between `<<untrusted:…>>` markers) only the address places. `Content`:
    /// everything. `Strict`: also other hosts, words, and encoded names withheld.
    pub fn outbound(&self, text: &str) -> String {
        match self.level {
            MaskLevel::Off => text.to_string(),
            MaskLevel::Content => self.mask(text),
            MaskLevel::Strict => {
                for re in &self.hosts {
                    for c in re.captures_iter(text) {
                        self.learn(&c[1]);
                    }
                }
                self.withhold_encoded(&self.mask(text))
            }
            MaskLevel::Addresses => {
                let mut out = String::with_capacity(text.len());
                let mut rest = text;
                while let Some((before, framed, after)) = next_frame(rest) {
                    out.push_str(&self.mask(before));
                    out.push_str(&self.mask_addresses(framed));
                    rest = after;
                }
                out.push_str(&self.mask(rest));
                out
            }
        }
    }

    fn mask_addresses(&self, text: &str) -> String {
        let mut out = text.to_string();
        for place in &self.addresses {
            out = place
                .replace_all(&out, |m: &regex::Captures| self.mask(&m[0]))
                .into_owned();
        }
        out
    }

    /// Strict: base64 / base64url tokens that encode a real name (e.g. a JWT whose `iss`
    /// names the host) are withheld whole.
    fn withhold_encoded(&self, text: &str) -> String {
        let mut secrets: Vec<String> = self.sites.iter().map(|p| p.real.clone()).collect();
        secrets.extend(
            self.auto
                .lock()
                .expect("auto lock")
                .pairs
                .iter()
                .map(|p| p.real.clone()),
        );
        for w in &self.words {
            secrets.push(w.real.clone());
            secrets.push(w.real.to_lowercase());
        }
        let needles: Vec<String> = secrets
            .iter()
            .flat_map(|s| base64_needles(s.as_bytes()))
            .collect();
        let token_char = |c: char| c.is_ascii_alphanumeric() || "+/=_-.".contains(c);
        let mut out = text.to_string();
        for needle in &needles {
            while let Some(at) = out.find(needle.as_str()) {
                let start = out[..at]
                    .rfind(|c: char| !token_char(c))
                    .map_or(0, |i| i + 1);
                let end = out[at..]
                    .find(|c: char| !token_char(c))
                    .map_or(out.len(), |i| at + i);
                out.replace_range(
                    start..end,
                    "[withheld by masking: encoded target name; use repeat_flow]",
                );
            }
        }
        out
    }
}

/// The base64 and base64url text a name turns into at each of the three alignments,
/// without the edge characters that also depend on the bytes around it.
fn base64_needles(secret: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for offset in 0..3 {
        let mut bytes = vec![0u8; offset];
        bytes.extend_from_slice(secret);
        let encoded = STANDARD_NO_PAD.encode(&bytes);
        let start = (offset * 8).div_ceil(6);
        let end = (bytes.len() * 8) / 6;
        if end > start && end - start >= 6 {
            let needle = &encoded[start..end];
            out.push(needle.to_string());
            let url = URL_SAFE_NO_PAD.encode(&bytes)[start..end].to_string();
            if url != needle {
                out.push(url);
            }
        }
    }
    out
}

/// Splits off the first `<<untrusted:N>>…<</untrusted:N>>` frame (markers stay outside).
fn next_frame(text: &str) -> Option<(&str, &str, &str)> {
    let start = text.find("<<untrusted:")?;
    let nonce_end = start + text[start..].find(">>")? + 2;
    let nonce = &text[start + "<<untrusted:".len()..nonce_end - 2];
    let close = format!("<</untrusted:{nonce}>>");
    let end = nonce_end + text[nonce_end..].find(&close)?;
    Some((&text[..nonce_end], &text[nonce_end..end], &text[end..]))
}

/// Replaces `from` (case-insensitive) as a whole name: the character before may be a
/// dot (a subdomain) or the end of a percent escape (`%2F%2Fexample.com`) but no other
/// name character, and the one after none at all.
fn swap(text: &str, from: &Regex, to: &str) -> String {
    let name_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    swap_where(text, from, to, |head, after| {
        let b = head.as_bytes();
        let escaped = b.len() >= 3
            && b[b.len() - 3] == b'%'
            && b[b.len() - 2..].iter().all(u8::is_ascii_hexdigit);
        let before = head.chars().next_back().filter(|_| !escaped);
        !before.is_some_and(name_char) && !after.is_some_and(name_char)
    })
}

/// A word, case-insensitive, not inside a longer word (`acme` in `acme-prod`, not in `acmeco`).
fn swap_word(text: &str, from: &Regex, to: &str) -> String {
    let word_char = |c: char| c.is_alphanumeric();
    swap_where(text, from, to, |head, after| {
        !head.chars().next_back().is_some_and(word_char) && !after.is_some_and(word_char)
    })
}

fn swap_where(
    text: &str,
    from: &Regex,
    to: &str,
    whole: impl Fn(&str, Option<char>) -> bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for m in from.find_iter(text) {
        if !whole(&text[..m.start()], text[m.end()..].chars().next()) {
            continue;
        }
        out.push_str(&text[copied..m.start()]);
        out.push_str(to);
        copied = m.end();
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(real: &str) -> MaskSite {
        MaskSite {
            real: real.into(),
            alias: String::new(),
        }
    }

    fn masker(level: MaskLevel) -> Masker {
        let masking = Masking {
            level,
            sites: vec![site("Example.com")],
            ..Masking::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(masking.sites[0].alias, "target1.masked");
        Masker::new(&masking)
    }

    #[test]
    fn content_masks_every_occurrence_and_only_whole_names() {
        let m = masker(MaskLevel::Content);
        assert_eq!(
            m.outbound("GET https://API.example.com/x  admin@example.com, example.com."),
            "GET https://API.target1.masked/x  admin@target1.masked, target1.masked."
        );
        for other in ["notexample.com", "example.community", "example.com-cdn.net"] {
            assert_eq!(m.outbound(other), other, "{other}");
        }
        assert!(
            !m.outbound("Welcome to EXAMPLE.COM")
                .to_lowercase()
                .contains("example.com")
        );
    }

    #[test]
    fn addresses_mask_shiftwires_words_and_addresses_in_target_content() {
        let m = masker(MaskLevel::Addresses);
        let text = "id 1 · GET https://example.com/a\n<<untrusted:ab12>>\nGET /a HTTP/1.1\nHost: example.com\nX-Note: example.com\n\n<a href=\"https://www.example.com/b\">Welcome to example.com</a> mail admin@example.com, json \"https:\\/\\/example.com\\/c\" next=https%3A%2F%2Fexample.com\n<</untrusted:ab12>>\nScope: example.com";
        let out = m.outbound(text);
        assert!(out.starts_with("id 1 · GET https://target1.masked/a"));
        assert!(out.contains("Host: target1.masked"));
        assert!(
            out.contains("X-Note: example.com"),
            "other headers are content"
        );
        assert!(out.contains("https://www.target1.masked/b"));
        assert!(
            out.contains("Welcome to example.com"),
            "free text stays at this level"
        );
        assert!(out.contains("admin@target1.masked"));
        assert!(out.contains(r"https:\/\/target1.masked\/c"));
        assert!(out.contains("next=https%3A%2F%2Ftarget1.masked"));
        assert!(out.ends_with("Scope: target1.masked"));
    }

    #[test]
    fn inbound_translates_aliases_in_every_argument() {
        let m = masker(MaskLevel::Addresses);
        let args = json!({"url": "https://api.Target1.masked/x?next=https%3A%2F%2Ftarget1.masked", "list": ["host:target1.masked"], "n": 3});
        assert_eq!(
            m.inbound(&args),
            json!({"url": "https://api.example.com/x?next=https%3A%2F%2Fexample.com", "list": ["host:example.com"], "n": 3})
        );
        let off = masker(MaskLevel::Off);
        assert_eq!(off.inbound(&args), args, "off: nothing changes");
        assert_eq!(off.outbound("example.com"), "example.com");
    }

    #[test]
    fn strict_masks_words_and_other_hosts_and_keeps_their_aliases() {
        let masking = Masking {
            level: MaskLevel::Strict,
            sites: vec![site("example.com")],
            words: vec!["Acme".into(), "Jane Doe".into()],
            allow: vec!["*.gstatic.com".into()],
            auto: vec![MaskSite {
                real: "acme.zendesk.com".into(),
                alias: "h1.masked".into(),
            }],
        }
        .normalized()
        .unwrap();
        let m = Masker::new(&masking);
        let out = m.outbound(
            "<<untrusted:ab>>Welcome to ACME (acme-prod) by Jane Doe; acmeco stays. <script src=\"https://cdn.partner.net/a.js\"></script> <img src=\"https://fonts.gstatic.com/x\"> help: https://acme.zendesk.com/new mail: ops@partner.net<</untrusted:ab>>",
        );
        assert!(
            out.contains("Welcome to [word1] ([word1]-prod) by [word2]; acmeco stays."),
            "{out}"
        );
        assert!(
            out.contains("https://h2.masked/a.js"),
            "a new host gets the next alias: {out}"
        );
        assert!(
            out.contains("ops@h3.masked"),
            "another host, its own alias: {out}"
        );
        assert!(
            out.contains("https://fonts.gstatic.com/x"),
            "allowed hosts stay: {out}"
        );
        assert!(out.contains("https://h1.masked/new"), "kept aliases: {out}");
        assert!(m.take_new_aliases() && !m.take_new_aliases(), "saved once");
        assert!(
            m.auto_sites()
                .iter()
                .any(|a| a.real == "cdn.partner.net" && a.alias == "h2.masked")
        );
        assert_eq!(
            m.inbound(&json!(
                "open https://h2.masked/a.js for [word1] and [word2]"
            )),
            json!("open https://cdn.partner.net/a.js for Acme and Jane Doe")
        );
    }

    #[test]
    fn strict_withholds_encoded_names() {
        let m = Masker::new(
            &Masking {
                level: MaskLevel::Strict,
                sites: vec![site("example.com")],
                ..Masking::default()
            }
            .normalized()
            .unwrap(),
        );
        // A JWT whose payload says {"iss":"https://login.example.com"}, at every alignment.
        for prefix in ["", "x", "xy"] {
            let payload = URL_SAFE_NO_PAD
                .encode(format!("{prefix}{{\"iss\":\"https://login.example.com\"}}"));
            let text =
                format!("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.{payload}.c2ln and more");
            let out = m.outbound(&text);
            assert_eq!(
                out,
                "Authorization: Bearer [withheld by masking: encoded target name; use repeat_flow] and more",
                "{prefix}"
            );
        }
        let plain = "dGhpcyBpcyBmaW5l"; // "this is fine"
        assert_eq!(m.outbound(plain), plain);
    }

    #[test]
    fn sites_words_and_aliases_are_checked() {
        let bad = |real: &str, alias: &str| {
            Masking {
                level: MaskLevel::Content,
                sites: vec![MaskSite {
                    real: real.into(),
                    alias: alias.into(),
                }],
                ..Masking::default()
            }
            .normalized()
            .is_err()
        };
        assert!(bad("not a host", ""));
        assert!(bad("example.com", "target1.com"), "aliases end in .masked");
        assert!(bad("example.com", "h3.masked"), "h<n> is for Strict");
        assert!(bad("x.masked", ""));
        let overlap = Masking {
            level: MaskLevel::Content,
            sites: vec![site("example.com"), site("api.example.com")],
            ..Masking::default()
        };
        assert!(overlap.normalized().is_err());
        let two = Masking {
            level: MaskLevel::Content,
            sites: vec![
                site("*.a.test"),
                MaskSite {
                    real: "b.test".into(),
                    alias: "target1.masked".into(),
                },
            ],
            words: vec!["  ".into(), "Acme".into(), "acme".into()],
            auto: vec![MaskSite {
                real: "api.a.test".into(),
                alias: "h1.masked".into(),
            }],
            ..Masking::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(
            (two.sites[0].real.as_str(), two.sites[0].alias.as_str()),
            ("a.test", "target2.masked")
        );
        assert_eq!(two.words, ["Acme"], "trimmed, no duplicates");
        assert!(two.auto.is_empty(), "a site covers it now");
        let short = Masking {
            words: vec!["ab".into()],
            ..Masking::default()
        };
        assert!(short.normalized().is_err());
    }
}
