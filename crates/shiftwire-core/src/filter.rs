//! The filter language, shared by the UI, the REST API and (later) MCP.
//!
//! ```text
//! host:shop.test status:>=400 -method:OPTIONS (tag:idor OR color:red) "password"
//! ```
//!
//! * Terms are ANDed; `OR`, `-`/`NOT` and parentheses work as expected.
//! * Text fields match a substring (case-insensitive); `*` is a wildcard and
//!   a leading `=` asks for an exact match: `host:=api.shop.test`, `host:*.shop.test`.
//! * Number fields take `>`, `>=`, `<`, `<=`: `status:>=400`, `status:4xx`, `size:>10k`, `ms:>500`,
//!   and wildcards on the digits: `status:2*`, `id:12*`.
//! * `is:error`, `is:noted`, `is:tagged`, `is:ai`, `is:tunnel`, `is:marked`.
//! * `id:` is the `#` in History: `id:>120` hides #1-#120 (History's Clear button).
//! * `ts:` is when the request started, in unix milliseconds: `ts:>=1700000000000`
//!   (History's "around this request" and "burst" views).
//! * Bare words search everything a History row shows: actor, method,
//!   host:port, path and query, status, and the tags.
//!
//! One parser, two evaluators: [`Filter::to_sql`] for stored history and
//! [`Filter::matches`] for live events. A test checks that they always agree.

use std::fmt;

use crate::model::FlowSummary;

pub const FIELDS: &str =
    "host, path, method, status, actor, via, tag, color, mime, note, size, ms, id, intent, is";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter(Option<Node>);

#[derive(Debug, Clone, PartialEq)]
pub struct FilterError(pub String);

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterError {}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    Text(TextField, Glob),
    Num(NumField, Op, u64),
    /// Wildcard on the decimal digits: `status:2*`.
    NumGlob(NumField, Glob),
    Tag(Glob),
    Is(Flag),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TextField {
    /// Bare words: everything shown in a History row.
    Any,
    Host,
    Path,
    Method,
    Actor,
    Color,
    Mime,
    Note,
    /// The agent's log_intent id a request was sent under.
    Intent,
    /// Which way it came in: proxy, tunnel, repeater, mcp, invisible.
    Via,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NumField {
    Status,
    Size,
    Ms,
    /// The `#` in History.
    Id,
    /// Start time, unix milliseconds.
    Ts,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Flag {
    Error,
    Noted,
    Tagged,
    Ai,
    Tunnel,
    Marked,
}

/// What bare words search: the text of a History row. `row_text` and
/// `ROW_TEXT_SQL` must build the same string (a test checks they agree).
fn row_text(f: &FlowSummary) -> String {
    let status = f.status.map(|s| s.to_string()).unwrap_or_default();
    format!(
        "{} {} {}:{} {} {status}",
        f.actor, f.method, f.host, f.port, f.path
    )
}

/// [`crate::model::via`] in SQL.
pub(crate) const VIA_SQL: &str =
    "CASE WHEN source LIKE 'invisible:%' OR source = 'redirect' THEN 'invisible' ELSE source END";

const ROW_TEXT_SQL: &str = "actor || ' ' || method || ' ' || host || ':' || port || ' ' || path     || COALESCE('?' || query, '') || ' ' || COALESCE(status, '')";

/// A lowercase pattern where `*` matches any run of characters.
#[derive(Debug, Clone, PartialEq)]
struct Glob(String);

impl Glob {
    fn substring(value: &str) -> Self {
        Self(format!("*{}*", value.to_ascii_lowercase()))
    }

    /// `=x` exact, `a*b` wildcard, otherwise substring.
    fn from_value(value: &str, exact_by_default: bool) -> Self {
        match value.strip_prefix('=') {
            Some(exact) => Self(exact.to_ascii_lowercase()),
            None if value.contains('*') || exact_by_default => Self(value.to_ascii_lowercase()),
            None => Self::substring(value),
        }
    }

    fn matches(&self, text: &str) -> bool {
        glob(self.0.as_bytes(), text.to_ascii_lowercase().as_bytes())
    }

    /// The same pattern for SQL `LIKE ... ESCAPE '\'`.
    fn like(&self) -> String {
        let mut out = String::with_capacity(self.0.len());
        for c in self.0.chars() {
            match c {
                '*' => out.push('%'),
                '%' | '_' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                c => out.push(c),
            }
        }
        out
    }
}

/// Iterative wildcard match (`*` only), linear in practice.
fn glob(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pat.len() && pat[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pat.len() && pat[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|&c| c == b'*')
}

impl Filter {
    pub fn parse(input: &str) -> Result<Self, FilterError> {
        let tokens = tokenize(input)?;
        if tokens.is_empty() {
            return Ok(Self(None));
        }
        let mut parser = Parser { tokens, pos: 0 };
        let node = parser.or()?;
        if parser.pos < parser.tokens.len() {
            return Err(FilterError("unexpected ')'".into()));
        }
        Ok(Self(Some(node)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn matches(&self, f: &FlowSummary) -> bool {
        self.0.as_ref().is_none_or(|n| n.matches(f))
    }

    /// A `WHERE` clause over the `flows` table, with its parameters.
    pub fn to_sql(&self) -> (String, Vec<String>) {
        let mut params = Vec::new();
        let sql = match &self.0 {
            Some(node) => node.sql(&mut params),
            None => "1".into(),
        };
        (sql, params)
    }
}

impl Node {
    fn matches(&self, f: &FlowSummary) -> bool {
        match self {
            Node::And(a, b) => a.matches(f) && b.matches(f),
            Node::Or(a, b) => a.matches(f) || b.matches(f),
            Node::Not(a) => !a.matches(f),
            Node::Text(field, glob) => {
                let row;
                let text = match field {
                    TextField::Any => {
                        row = row_text(f);
                        row.as_str()
                    }
                    TextField::Host => &f.host,
                    TextField::Path => &f.path,
                    TextField::Method => &f.method,
                    TextField::Actor => &f.actor,
                    TextField::Color => f.color.as_deref().unwrap_or(""),
                    TextField::Mime => f.mime.as_deref().unwrap_or(""),
                    TextField::Note => f.note.as_deref().unwrap_or(""),
                    TextField::Intent => f.intent.as_deref().unwrap_or(""),
                    TextField::Via => &f.via,
                };
                glob.matches(text)
                    || (*field == TextField::Any && f.tags.iter().any(|t| glob.matches(t)))
            }
            Node::NumGlob(field, glob) => glob.matches(&field.value(f).to_string()),
            Node::Num(field, op, want) => {
                let have = field.value(f);
                match op {
                    Op::Eq => have == *want,
                    Op::Lt => have < *want,
                    Op::Le => have <= *want,
                    Op::Gt => have > *want,
                    Op::Ge => have >= *want,
                }
            }
            Node::Tag(glob) => f.tags.iter().any(|t| glob.matches(t)),
            Node::Is(flag) => match flag {
                Flag::Error => f.error.is_some(),
                Flag::Noted => f.note.as_deref().is_some_and(|n| !n.is_empty()),
                Flag::Tagged => !f.tags.is_empty(),
                Flag::Ai => f.actor != "proxy",
                Flag::Tunnel => f.method == "CONNECT",
                Flag::Marked => f.mark.is_some(),
            },
        }
    }

    fn sql(&self, params: &mut Vec<String>) -> String {
        const TAGS: &str = "SELECT 1 FROM tags WHERE tags.flow_id = flows.id";
        match self {
            Node::And(a, b) => format!("({} AND {})", a.sql(params), b.sql(params)),
            Node::Or(a, b) => format!("({} OR {})", a.sql(params), b.sql(params)),
            Node::Not(a) => format!("(NOT {})", a.sql(params)),
            Node::Text(field, glob) => {
                let column = match field {
                    TextField::Any => ROW_TEXT_SQL,
                    TextField::Host => "host",
                    TextField::Path => "path || COALESCE('?' || query, '')",
                    TextField::Method => "method",
                    TextField::Actor => "actor",
                    TextField::Color => "COALESCE(color, '')",
                    TextField::Mime => "COALESCE(mime, '')",
                    TextField::Note => "COALESCE(note, '')",
                    TextField::Intent => "COALESCE(intent, '')",
                    TextField::Via => VIA_SQL,
                };
                params.push(glob.like());
                let n = params.len();
                if *field == TextField::Any {
                    // Tags are shown in the row too, so bare words find them.
                    format!(
                        "(LOWER({column}) LIKE ?{n} ESCAPE '\\' \
                         OR EXISTS ({TAGS} AND LOWER(tags.tag) LIKE ?{n} ESCAPE '\\'))"
                    )
                } else {
                    format!("(LOWER({column}) LIKE ?{n} ESCAPE '\\')")
                }
            }
            Node::NumGlob(field, glob) => {
                params.push(glob.like());
                format!("(CAST({} AS TEXT) LIKE ?{} ESCAPE '\\')", field.column(), params.len())
            }
            Node::Num(field, op, value) => {
                let column = field.column();
                let op = match op {
                    Op::Eq => "=",
                    Op::Lt => "<",
                    Op::Le => "<=",
                    Op::Gt => ">",
                    Op::Ge => ">=",
                };
                format!("({column} {op} {value})") // a parsed integer: safe to inline
            }
            Node::Tag(glob) => {
                params.push(glob.like());
                format!(
                    "EXISTS ({TAGS} AND LOWER(tags.tag) LIKE ?{} ESCAPE '\\')",
                    params.len()
                )
            }
            Node::Is(flag) => match flag {
                Flag::Error => "(error IS NOT NULL)".into(),
                Flag::Noted => "(COALESCE(note, '') != '')".into(),
                Flag::Tagged => format!("EXISTS ({TAGS})"),
                Flag::Ai => "(actor != 'proxy')".into(),
                Flag::Tunnel => "(method = 'CONNECT')".into(),
                Flag::Marked => "EXISTS (SELECT 1 FROM timeline WHERE timeline.flow_id = flows.id AND kind = 'mark')".into(),
            },
        }
    }
}

// ---- tokens & parser --------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Tok {
    Open,
    Close,
    /// `quoted`: the whole word was in quotes, so it is never a keyword or field.
    Word {
        text: String,
        quoted: bool,
    },
}

fn tokenize(input: &str) -> Result<Vec<Tok>, FilterError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' | ')' => {
                tokens.push(if c == '(' { Tok::Open } else { Tok::Close });
                chars.next();
            }
            _ => {
                // A word runs to the next space or paren outside quotes; quotes are dropped.
                let (mut text, mut in_quotes, mut unquoted_chars) = (String::new(), false, false);
                while let Some(&c) = chars.peek() {
                    if c == '"' {
                        in_quotes = !in_quotes;
                    } else if !in_quotes && (c.is_whitespace() || c == '(' || c == ')') {
                        break;
                    } else {
                        unquoted_chars |= !in_quotes;
                        text.push(c);
                    }
                    chars.next();
                }
                if in_quotes {
                    return Err(FilterError("missing closing quote".into()));
                }
                tokens.push(Tok::Word {
                    text,
                    quoted: !unquoted_chars,
                });
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek_keyword(&self, keyword: &str) -> bool {
        matches!(self.tokens.get(self.pos), Some(Tok::Word { text, quoted: false }) if text == keyword)
    }

    fn or(&mut self) -> Result<Node, FilterError> {
        let mut node = self.and()?;
        while self.peek_keyword("OR") {
            self.pos += 1;
            node = Node::Or(Box::new(node), Box::new(self.and()?));
        }
        Ok(node)
    }

    fn and(&mut self) -> Result<Node, FilterError> {
        let mut node = self.unary()?;
        loop {
            if self.peek_keyword("AND") {
                self.pos += 1;
            }
            match self.tokens.get(self.pos) {
                None | Some(Tok::Close) => return Ok(node),
                _ if self.peek_keyword("OR") => return Ok(node),
                _ => node = Node::And(Box::new(node), Box::new(self.unary()?)),
            }
        }
    }

    fn unary(&mut self) -> Result<Node, FilterError> {
        if self.peek_keyword("NOT") || self.peek_keyword("-") {
            self.pos += 1;
            return Ok(Node::Not(Box::new(self.unary()?)));
        }
        match self.tokens.get(self.pos) {
            None => Err(FilterError("filter ends too early".into())),
            Some(Tok::Close) => Err(FilterError("unexpected ')'".into())),
            Some(Tok::Open) => {
                self.pos += 1;
                let node = self.or()?;
                if self.tokens.get(self.pos) != Some(&Tok::Close) {
                    return Err(FilterError("missing ')'".into()));
                }
                self.pos += 1;
                Ok(node)
            }
            Some(Tok::Word { text, quoted }) => {
                let (text, quoted) = (text.clone(), *quoted);
                self.pos += 1;
                if !quoted && let Some(rest) = text.strip_prefix('-').filter(|r| !r.is_empty()) {
                    return Ok(Node::Not(Box::new(term(rest)?)));
                }
                if quoted {
                    Ok(Node::Text(TextField::Any, Glob::substring(&text)))
                } else {
                    term(&text)
                }
            }
        }
    }
}

fn term(word: &str) -> Result<Node, FilterError> {
    let Some((key, value)) = word.split_once(':') else {
        return Ok(Node::Text(TextField::Any, Glob::substring(word)));
    };
    let text = |field, exact| Ok(Node::Text(field, Glob::from_value(value, exact)));
    match key.to_ascii_lowercase().as_str() {
        "host" => text(TextField::Host, false),
        "path" | "url" => text(TextField::Path, false),
        "method" => text(TextField::Method, true),
        "actor" => text(TextField::Actor, false),
        "color" => text(TextField::Color, true),
        "mime" | "type" => text(TextField::Mime, false),
        "note" => text(TextField::Note, false),
        "intent" => text(TextField::Intent, true),
        "via" => text(TextField::Via, true),
        "tag" => Ok(Node::Tag(Glob::from_value(value, true))),
        "status" => number(NumField::Status, value),
        "size" => number(NumField::Size, value),
        "ms" | "time" => number(NumField::Ms, value),
        "id" | "#" => number(NumField::Id, value),
        "ts" => number(NumField::Ts, value),
        "is" | "has" => match value.to_ascii_lowercase().as_str() {
            "error" | "failed" => Ok(Node::Is(Flag::Error)),
            "noted" | "note" => Ok(Node::Is(Flag::Noted)),
            "tagged" | "tag" => Ok(Node::Is(Flag::Tagged)),
            "ai" | "agent" => Ok(Node::Is(Flag::Ai)),
            "tunnel" => Ok(Node::Is(Flag::Tunnel)),
            "marked" | "important" => Ok(Node::Is(Flag::Marked)),
            other => Err(FilterError(format!(
                "unknown is:{other} (use error, noted, tagged, ai, tunnel, marked)"
            ))),
        },
        // `http://…` and similar: not a field, just text.
        k if !k.chars().all(|c| c.is_ascii_alphabetic()) || value.starts_with("//") => {
            Ok(Node::Text(TextField::Any, Glob::substring(word)))
        }
        other => Err(FilterError(format!(
            "unknown field '{other}' (fields: {FIELDS})"
        ))),
    }
}

impl NumField {
    fn value(self, f: &FlowSummary) -> u64 {
        match self {
            NumField::Status => u64::from(f.status.unwrap_or(0)),
            NumField::Size => f.size,
            NumField::Ms => f.duration_ms,
            NumField::Id => f.seq,
            NumField::Ts => f.ts,
        }
    }

    fn column(self) -> &'static str {
        match self {
            NumField::Status => "COALESCE(status, 0)",
            NumField::Size => "resp_size",
            NumField::Ms => "(ts_end - ts_start)",
            NumField::Id => "seq",
            NumField::Ts => "ts_start",
        }
    }
}

fn number(field: NumField, value: &str) -> Result<Node, FilterError> {
    let bad = || FilterError(format!("'{value}' is not a number"));
    if value.contains('*') {
        if !value.chars().all(|c| c.is_ascii_digit() || c == '*') {
            return Err(bad());
        }
        return Ok(Node::NumGlob(field, Glob(value.to_string())));
    }
    if field == NumField::Status
        && let Some(class) = value.strip_suffix("xx").and_then(|d| d.parse::<u64>().ok())
    {
        let lo = Node::Num(field, Op::Ge, class * 100);
        let hi = Node::Num(field, Op::Lt, class * 100 + 100);
        return Ok(Node::And(Box::new(lo), Box::new(hi)));
    }
    let (op, rest) = [
        (">=", Op::Ge),
        ("<=", Op::Le),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ]
    .into_iter()
    .find_map(|(s, op)| value.strip_prefix(s).map(|r| (op, r)))
    .unwrap_or((Op::Eq, value));
    let lower = rest.to_ascii_lowercase();
    let (digits, scale) = match lower.as_bytes().last() {
        Some(b'k') => (&lower[..lower.len() - 1], 1024),
        Some(b'm') if field == NumField::Size => (&lower[..lower.len() - 1], 1024 * 1024),
        _ => (lower.as_str(), 1),
    };
    let n: u64 = digits.trim().parse().map_err(|_| bad())?;
    Ok(Node::Num(field, op, n * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(host: &str, path: &str, status: Option<u16>) -> FlowSummary {
        FlowSummary {
            id: "x".into(),
            seq: 7,
            ts: 0,
            actor: "proxy".into(),
            method: "GET".into(),
            scheme: "https".into(),
            host: host.into(),
            port: 443,
            path: path.into(),
            status,
            mime: Some("application/json".into()),
            size: 2048,
            duration_ms: 120,
            error: None,
            tags: vec![],
            color: None,
            note: None,
            mark: None,
            mark_reason: None,
            intent: Some("01INTENT".into()),
            via: "proxy".into(),
        }
    }

    fn m(filter: &str, f: &FlowSummary) -> bool {
        Filter::parse(filter).unwrap().matches(f)
    }

    #[test]
    fn fields_and_operators() {
        let f = flow("api.shop.test", "/users/7?admin=1", Some(403));
        assert!(m("id:7 id:>6 #:<=7", &f), "id: is the # in History");
        assert!(
            m("intent:01intent", &f) && !m("intent:01int", &f),
            "intent: is exact"
        );
        assert!(
            m("status:4* status:*3 -status:2*", &f),
            "wildcards on numbers"
        );
        assert!(Filter::parse("status:2x*").is_err());
        assert!(m("", &f));
        assert!(m("host:shop.test", &f));
        assert!(m("host:*.shop.test", &f));
        assert!(!m("host:=shop.test", &f));
        assert!(m("status:>=400 status:4xx", &f));
        assert!(!m("status:5xx", &f));
        assert!(m("size:>1k size:<=2k ms:>100", &f));
        assert!(m("ts:>=0 ts:<=0", &f) && !m("ts:>0", &f));
        assert!(m("admin", &f), "bare words search the whole row");
        let mut tagged = f.clone();
        tagged.tags = vec!["recon".into()];
        assert!(
            m("recon", &tagged) && m("reco*", &tagged),
            "bare words find tags"
        );
        assert!(!m("recon", &f));
        assert!(m("443", &f) && m("proxy", &f) && m("403", &f) && m("get", &f));
        assert!(m("-method:POST", &f));
        assert!(m("method:get", &f), "case-insensitive");
        assert!(!m("method:GE", &f), "method is exact");
        assert!(m("mime:json", &f));
    }

    #[test]
    fn boolean_structure() {
        let f = flow("a.test", "/login", Some(200));
        assert!(m("host:b.test OR path:login", &f));
        assert!(!m("host:b.test OR path:logout", &f));
        assert!(m("(host:b.test OR host:a.test) status:200", &f));
        assert!(!m("NOT (host:a.test)", &f));
        assert!(m("- host:b.test", &f));
        assert!(
            m("\"OR\" OR login", &f),
            "a quoted OR is a word, not an operator"
        );
    }

    #[test]
    fn tags_colors_notes_and_flags() {
        let mut f = flow("a.test", "/", None);
        f.tags = vec!["idor".into(), "auth-bypass".into()];
        f.color = Some("red".into());
        f.note = Some("check this".into());
        f.actor = "claude".into();
        f.error = Some("refused".into());
        assert!(m("tag:idor color:red note:check", &f));
        assert!(m("tag:auth*", &f));
        assert!(
            !m("tag:auth", &f),
            "tags match whole names unless wildcarded"
        );
        assert!(m("is:tagged is:noted is:ai is:error", &f));
        assert!(m("status:0", &f), "no status counts as 0");
        assert!(m("via:proxy", &f));
        f.via = "invisible".into(); // old rows from the removed invisible proxy
        assert!(m("via:invisible", &f) && !m("via:invis", &f));
    }

    /// What the UI sends for lasting filters: main filter, "show only" chips ORed, "hide" chips excluded.
    #[test]
    fn lasting_filter_chips_combine() {
        let telemetry = flow("incoming.telemetry.mozilla.org", "/submit", Some(200));
        let shop = flow("shop.test", "/api/cart", Some(200));
        let chips = r#"(status:200) ("shop.test" OR "other.test") -"*mozilla*" -"firefox""#;
        assert!(!m(chips, &telemetry));
        assert!(m(chips, &shop));
        assert!(
            !m(r#"-"shop.test:443""#, &shop),
            "chips match host:port as shown"
        );
    }

    #[test]
    fn helpful_errors() {
        assert!(
            Filter::parse("hots:x")
                .unwrap_err()
                .0
                .contains("unknown field 'hots'")
        );
        assert!(Filter::parse("status:abc").is_err());
        assert!(Filter::parse("(host:a").is_err());
        assert!(Filter::parse("host:a)").is_err());
        assert!(Filter::parse("\"open").is_err());
        assert!(
            Filter::parse("http://a.test/x").is_ok(),
            "URLs are text, not fields"
        );
    }

    #[test]
    fn glob_edge_cases() {
        assert!(glob(b"*", b""));
        assert!(glob(b"a*c", b"abbbc"));
        assert!(!glob(b"a*c", b"abbbd"));
        assert!(glob(b"*a*a*", b"banana"));
        assert_eq!(Glob::substring("50%_off").like(), "%50\\%\\_off%");
    }
}
