//! Native JSONL adapters. Only allowlisted accounting metadata leaves this module.
use super::models::{Provider, Tokens, safe_label};
use serde_json::Value;

#[derive(Clone)]
pub(super) enum Event {
    Model(String),
    NativeSession(String),
    Usage {
        key: Option<String>,
        native_session: Option<String>,
        model: Option<String>,
        tokens: Tokens,
        cumulative: bool,
    },
}

pub(super) struct Parsed {
    pub event: Option<Event>,
    pub timestamp: Option<String>,
    pub diagnostics: Vec<&'static str>,
    pub snapshot: Option<Event>,
}

fn label(v: &Value) -> Option<String> {
    v.as_str().filter(|s| safe_label(s)).map(str::to_owned)
}

fn number(v: &Value, field: &str, diagnostics: &mut Vec<&'static str>) -> u64 {
    match v[field].as_u64() {
        Some(n) => n,
        None => {
            diagnostics.push("missing_or_invalid_token_field");
            0
        }
    }
}

pub(super) fn parse(provider: Provider, v: &Value) -> Parsed {
    let mut p = Parsed {
        event: None,
        timestamp: None,
        diagnostics: Vec::new(),
        snapshot: None,
    };
    p.timestamp = v["timestamp"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc).to_rfc3339());
    match provider {
        Provider::Claude => claude(v, &mut p),
        Provider::Codex => codex(v, &mut p),
    }
    if matches!(p.event, Some(Event::Usage { .. })) && p.timestamp.is_none() {
        p.diagnostics.push("missing_timestamp");
    }
    p
}

fn claude(v: &Value, p: &mut Parsed) {
    if v["type"] != "assistant" {
        return;
    }
    let msg = &v["message"];
    let u = &msg["usage"];
    if !u.is_object() {
        p.diagnostics.push("missing_usage");
        return;
    }
    let d = &mut p.diagnostics;
    let write = number(u, "cache_creation_input_tokens", d);
    let parts = &u["cache_creation"];
    let (five, hour) = if parts.is_object() {
        (
            number(parts, "ephemeral_5m_input_tokens", d),
            number(parts, "ephemeral_1h_input_tokens", d),
        )
    } else {
        if write > 0 {
            d.push("unknown_cache_write_duration");
        }
        (0, 0)
    };
    if five.saturating_add(hour) > write {
        d.push("inconsistent_cache_partitions");
    }
    let t = Tokens {
        input: number(u, "input_tokens", d),
        cache_write: write.max(five.saturating_add(hour)),
        cache_write_5m: five,
        cache_write_1h: hour,
        cache_read: number(u, "cache_read_input_tokens", d),
        output: number(u, "output_tokens", d),
        reasoning: 0,
    };
    let key = label(&msg["id"]);
    if key.is_none() {
        d.push("missing_message_identity");
    }
    p.event = Some(Event::Usage {
        key,
        native_session: label(&v["sessionId"]),
        model: label(&msg["model"]),
        tokens: t,
        cumulative: false,
    });
}

fn codex_tokens(u: &Value, d: &mut Vec<&'static str>) -> Tokens {
    let input = number(u, "input_tokens", d);
    let read = number(u, "cached_input_tokens", d);
    if u.get("cache_write_input_tokens").is_none() {
        d.push("cache_write_counter_unreported");
    }
    let write = u
        .get("cache_write_input_tokens")
        .map(|_| number(u, "cache_write_input_tokens", d))
        .unwrap_or(0);
    let output = number(u, "output_tokens", d);
    let reasoning = number(u, "reasoning_output_tokens", d);
    if read.saturating_add(write) > input || reasoning > output {
        d.push("inconsistent_token_subsets");
    }
    if write > 0 {
        d.push("unknown_cache_write_duration");
    }
    Tokens {
        input,
        cache_read: read,
        cache_write: write,
        output,
        reasoning,
        ..Tokens::default()
    }
}

fn codex(v: &Value, p: &mut Parsed) {
    let payload = &v["payload"];
    if v["type"] == "session_meta" {
        p.event = label(&payload["id"]).map(Event::NativeSession);
        return;
    }
    if v["type"] == "turn_context" {
        p.event = label(&payload["model"]).map(Event::Model);
        return;
    }
    if v["type"] == "token_usage_record" {
        let native = label(&payload["thread_id"]).or_else(|| label(&payload["session_id"]));
        if payload["usage"].is_object() {
            p.event = Some(Event::Usage {
                key: label(&payload["response_id"]),
                native_session: native.clone(),
                model: label(&payload["model"]),
                tokens: codex_tokens(&payload["usage"], &mut p.diagnostics),
                cumulative: false,
            });
        } else {
            p.diagnostics.push("missing_usage");
        }
        if payload["thread_token_usage"].is_object() {
            p.snapshot = Some(Event::Usage {
                key: None,
                native_session: native,
                model: label(&payload["model"]),
                tokens: codex_tokens(&payload["thread_token_usage"], &mut p.diagnostics),
                cumulative: true,
            });
        }
        return;
    }
    if v["type"] != "event_msg" || payload["type"] != "token_count" {
        return;
    }
    let info = &payload["info"];
    if info.is_null() {
        return;
    }
    let total = &info["total_token_usage"];
    let cumulative = total.is_object();
    let u = if cumulative {
        total
    } else {
        &info["last_token_usage"]
    };
    if !u.is_object() {
        p.diagnostics.push("missing_usage");
        return;
    }
    p.event = Some(Event::Usage {
        key: label(&payload["response_id"])
            .or_else(|| label(&payload["request_id"]))
            .or_else(|| label(&info["request_id"])),
        native_session: None,
        model: label(&payload["model"]),
        tokens: codex_tokens(u, &mut p.diagnostics),
        cumulative,
    });
}
