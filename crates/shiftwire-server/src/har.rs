//! HAR 1.2 export: hand captured traffic to other tools (browsers, Burp, ZAP, scripts).

use serde_json::{Value, json};
use shiftwire_core::{App, Filter, FlowDetail, Message};

/// The flows matching `filter` (newest `limit`), as a HAR document.
pub fn export(app: &App, filter: &Filter, limit: usize) -> std::io::Result<Value> {
    let mut entries = Vec::new();
    for summary in app.query_flows(filter, limit)? {
        if summary.method == "CONNECT" {
            continue; // a tunnel has no HTTP exchange to export
        }
        if let Some(detail) = app.flow_detail(&summary.id)? {
            entries.push(entry(&detail));
        }
    }
    Ok(json!({ "log": {
        "version": "1.2",
        "creator": { "name": "shiftWire", "version": shiftwire_core::VERSION },
        "entries": entries,
    }}))
}

fn entry(d: &FlowDetail) -> Value {
    let f = &d.summary;
    let default_port =
        (f.scheme == "https" && f.port == 443) || (f.scheme == "http" && f.port == 80);
    let url = format!(
        "{}://{}{}{}",
        f.scheme,
        f.host,
        if default_port {
            String::new()
        } else {
            format!(":{}", f.port)
        },
        f.path
    );
    let query: Vec<Value> = f
        .path
        .split_once('?')
        .map(|(_, q)| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (n, v) = p.split_once('=').unwrap_or((p, ""));
                    json!({ "name": n, "value": v })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut request = json!({
        "method": f.method,
        "url": url,
        "httpVersion": d.http_version,
        "headers": headers(&d.request),
        "queryString": query,
        "cookies": [],
        "headersSize": -1,
        "bodySize": d.request.body.size,
    });
    if d.request.body.size > 0 {
        request["postData"] = json!({
            "mimeType": header(&d.request, "content-type").unwrap_or_default(),
            "text": d.request.body.text.clone().or_else(|| d.request.body.base64.clone()).unwrap_or_default(),
        });
    }
    let response = match &d.response {
        Some(r) => {
            let mut content = json!({
                "size": r.body.size,
                "mimeType": header(r, "content-type").unwrap_or_default(),
            });
            if let Some(text) = &r.body.text {
                content["text"] = json!(text);
            } else if let Some(b64) = &r.body.base64 {
                content["text"] = json!(b64);
                content["encoding"] = json!("base64");
            }
            let status_text = r
                .head
                .lines()
                .next()
                .and_then(|l| l.splitn(3, ' ').nth(2))
                .unwrap_or_default();
            json!({
                "status": f.status.unwrap_or(0),
                "statusText": status_text.trim(),
                "httpVersion": r.head.split_whitespace().next().unwrap_or("HTTP/1.1"),
                "headers": headers(r),
                "cookies": [],
                "content": content,
                "redirectURL": header(r, "location").unwrap_or_default(),
                "headersSize": -1,
                "bodySize": r.body.size,
            })
        }
        // HAR has no "no response": status 0 is the convention (browsers do the same).
        None => json!({
            "status": 0, "statusText": f.error.clone().unwrap_or_default(), "httpVersion": "", "headers": [],
            "cookies": [], "content": { "size": 0, "mimeType": "" }, "redirectURL": "", "headersSize": -1, "bodySize": 0,
        }),
    };
    let mut comment = vec![format!("actor: {}", f.actor)];
    if !f.tags.is_empty() {
        comment.push(format!("tags: {}", f.tags.join(", ")));
    }
    if let Some(n) = &f.note {
        comment.push(format!("note: {n}"));
    }
    json!({
        "startedDateTime": iso8601(f.ts),
        "time": f.duration_ms,
        "request": request,
        "response": response,
        "cache": {},
        "timings": { "send": 0, "wait": f.duration_ms, "receive": 0 },
        "comment": comment.join(" · "),
    })
}

fn headers(m: &Message) -> Vec<Value> {
    m.head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| json!({ "name": n.trim(), "value": v.trim() }))
        .collect()
}

fn header(m: &Message, name: &str) -> Option<String> {
    shiftwire_core::model::header(&m.head, name).map(str::to_string)
}

/// Unix milliseconds as `2026-11-17T08:22:13.123Z` (civil-from-days, no date library).
fn iso8601(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let (h, m, s) = (secs % 86_400 / 3600, secs % 3600 / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{:03}Z",
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_iso8601_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_794_902_775_819), "2026-11-17T08:06:15.819Z");
        assert_eq!(iso8601(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
