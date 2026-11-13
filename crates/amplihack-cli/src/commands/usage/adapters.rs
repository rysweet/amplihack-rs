//! Native JSONL adapters. Only allowlisted accounting metadata leaves this module.
use super::models::{InputEvidence, Provider, Tokens, safe_label};
use serde_json::Value;

#[derive(Clone)]
pub(super) enum Event {
    Model(Option<String>),
    /// Every metadata boundary replaces the owner, even when its ID is invalid.
    NativeSession(Option<String>),
    Usage {
        key: Option<String>,
        native_session: Option<String>,
        model: Option<String>,
        tokens: Option<Tokens>,
        cumulative: bool,
        input: InputEvidence,
        last: LastUsage,
    },
    /// A record that may stand for a provider request but whose usage cannot
    /// be counted. It adds no response, attempt or tokens, but its owner's
    /// context input is unknown for `reason`, never a partial sum that looks
    /// exact. Every parsed record that is diagnosed and then not counted emits
    /// this (checked in `parse`), so its native owner is marked without a list
    /// of diagnostic codes. Lines that cannot be read or parsed as JSON never
    /// reach an adapter and have no native owner; the aggregator marks the
    /// session that lists them.
    UnknownInput {
        native_session: Option<String>,
        reason: &'static str,
    },
}

/// What a cumulative record says about the request that moved its total.
#[derive(Clone, Debug)]
pub(super) enum LastUsage {
    /// No `last_token_usage` beside the total. Responses, last-only records
    /// and `thread_token_usage` snapshots have none, so a positive high-water
    /// transition cannot be checked and is taken as one request.
    Unreported,
    /// Codex `token_count` reports the request behind each total. `None` means
    /// the field is present without a usable counter, so the transition
    /// cannot be confirmed as one request. Boxed so that every usage event
    /// does not grow by a second set of counters.
    Reported(Option<Box<Tokens>>),
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

fn number(v: &Value, field: &str, diagnostics: &mut Vec<&'static str>, observed: &mut bool) -> u64 {
    match v[field].as_u64() {
        Some(n) => {
            *observed = true;
            n
        }
        None => {
            diagnostics.push("missing_or_invalid_token_field");
            0
        }
    }
}

pub(super) fn parse(provider: &Provider, v: &Value) -> Parsed {
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
        Provider::Unsupported(_) => p.diagnostics.push("unsupported_provider"),
    }
    if (matches!(p.event, Some(Event::Usage { .. }))
        || matches!(p.snapshot, Some(Event::Usage { .. })))
        && p.timestamp.is_none()
    {
        p.diagnostics.push("missing_timestamp");
    }
    // A diagnosed record that reaches the aggregator as no event at all would
    // drop out of context validity unseen; it must be an `UnknownInput`.
    debug_assert!(
        matches!(provider, Provider::Unsupported(_))
            || p.diagnostics.is_empty()
            || p.event.is_some()
            || p.snapshot.is_some(),
        "a diagnosed record produced no event: {:?}",
        p.diagnostics
    );
    p
}

/// Claude Code writes its own `assistant` records for API errors and
/// interrupts: model `<synthetic>`, usually `isApiErrorMessage: true`, a
/// client UUID as `message.id` and explicit zero counters. They are not
/// provider responses, so they are no observation: counting one would add a
/// response with a known input of zero. A failed or interrupted call stays an
/// unrecorded call, unknown rather than zero. If such a record ever carries a
/// nonzero or non-numeric counter, nothing tells whether the provider served
/// it: it is diagnosed, and its native session's input becomes unknown.
fn client_generated(v: &Value) -> bool {
    v["message"]["model"] == "<synthetic>" || v["isApiErrorMessage"] == true
}

fn claude(v: &Value, p: &mut Parsed) {
    if v["type"] != "assistant" {
        return;
    }
    let msg = &v["message"];
    let u = &msg["usage"];
    if client_generated(v) {
        let counted = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ]
        .iter()
        .any(|field| !u[*field].is_null() && u[*field].as_u64() != Some(0));
        if counted {
            p.diagnostics.push("usage_on_client_generated_record");
            p.event = Some(Event::UnknownInput {
                native_session: label(&v["sessionId"]),
                reason: "usage_on_client_generated_record",
            });
        }
        return;
    }
    let tokens = if u.is_object() {
        claude_tokens(u, &mut p.diagnostics)
    } else {
        p.diagnostics.push("missing_usage");
        None
    };
    let key = label(&msg["id"]);
    if key.is_none() {
        p.diagnostics.push("missing_message_identity");
    }
    // Missing counters must not discard the owner of a copied response.
    p.event = Some(Event::Usage {
        key,
        native_session: label(&v["sessionId"]),
        model: label(&msg["model"]),
        tokens,
        cumulative: false,
        input: input_evidence(&Provider::Claude, u),
        last: LastUsage::Unreported,
    });
}

fn claude_tokens(u: &Value, d: &mut Vec<&'static str>) -> Option<Tokens> {
    let mut observed = false;
    let write = number(u, "cache_creation_input_tokens", d, &mut observed);
    let parts = &u["cache_creation"];
    let (five, hour) = if parts.is_object() {
        (
            number(parts, "ephemeral_5m_input_tokens", d, &mut observed),
            number(parts, "ephemeral_1h_input_tokens", d, &mut observed),
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
        input: number(u, "input_tokens", d, &mut observed),
        cache_write: write.max(five.saturating_add(hour)),
        cache_write_5m: five,
        cache_write_1h: hour,
        cache_read: number(u, "cache_read_input_tokens", d, &mut observed),
        output: number(u, "output_tokens", d, &mut observed),
        reasoning: 0,
    };
    observed.then_some(t)
}

fn codex_tokens(u: &Value, d: &mut Vec<&'static str>) -> Option<Tokens> {
    let mut observed = false;
    let input = number(u, "input_tokens", d, &mut observed);
    let read = number(u, "cached_input_tokens", d, &mut observed);
    if u.get("cache_write_input_tokens").is_none() {
        d.push("cache_write_counter_unreported");
    }
    let write = u
        .get("cache_write_input_tokens")
        .map(|_| number(u, "cache_write_input_tokens", d, &mut observed))
        .unwrap_or(0);
    let output = number(u, "output_tokens", d, &mut observed);
    let reasoning = number(u, "reasoning_output_tokens", d, &mut observed);
    if read.saturating_add(write) > input || reasoning > output {
        d.push("inconsistent_token_subsets");
    }
    if write > 0 {
        d.push("unknown_cache_write_duration");
    }
    observed.then_some(Tokens {
        input,
        cache_read: read,
        cache_write: write,
        output,
        reasoning,
        ..Tokens::default()
    })
}

fn codex(v: &Value, p: &mut Parsed) {
    let payload = &v["payload"];
    if v["type"] == "session_meta" {
        let native = label(&payload["id"]);
        if native.is_none() {
            p.diagnostics.push("missing_or_invalid_session_identity");
        }
        p.event = Some(Event::NativeSession(native));
        return;
    }
    if v["type"] == "turn_context" {
        let model = label(&payload["model"]);
        if model.is_none() {
            p.diagnostics.push("missing_or_invalid_model");
        }
        // Every turn replaces the prior context, including an invalid one.
        p.event = Some(Event::Model(model));
        return;
    }
    if v["type"] == "token_usage_record" {
        let native = label(&payload["thread_id"]).or_else(|| label(&payload["session_id"]));
        // A pure thread snapshot is not an unidentified response observation.
        // Explicit but invalid response metadata must still remain visible.
        if payload.get("response_id").is_some()
            || payload.get("usage").is_some()
            || !payload["thread_token_usage"].is_object()
        {
            let tokens = if payload["usage"].is_object() {
                codex_tokens(&payload["usage"], &mut p.diagnostics)
            } else {
                p.diagnostics.push("missing_usage");
                None
            };
            p.event = Some(Event::Usage {
                key: label(&payload["response_id"]),
                native_session: native.clone(),
                model: label(&payload["model"]),
                tokens,
                cumulative: false,
                input: input_evidence(&Provider::Codex, &payload["usage"]),
                last: LastUsage::Unreported,
            });
        }
        if payload["thread_token_usage"].is_object() {
            p.snapshot = Some(Event::Usage {
                key: None,
                native_session: native,
                model: label(&payload["model"]),
                tokens: codex_tokens(&payload["thread_token_usage"], &mut p.diagnostics),
                cumulative: true,
                input: input_evidence(&Provider::Codex, &payload["thread_token_usage"]),
                last: LastUsage::Unreported,
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
    let last_usage = &info["last_token_usage"];
    let cumulative = total.is_object();
    let u = if cumulative { total } else { last_usage };
    if !u.is_object() {
        // `info` is present, so a request may have moved the totals; with
        // neither total nor last usage, its input is unknown, not zero.
        p.diagnostics.push("missing_usage");
        p.event = Some(Event::UnknownInput {
            native_session: None,
            reason: "missing_input_fields",
        });
        return;
    }
    // Beside a total, `last_token_usage` is the request that moved it. The
    // aggregator compares it with the high-water delta, so a missing prefix
    // or a gap between files is not counted as a single request.
    let last = if !cumulative || last_usage.is_null() {
        LastUsage::Unreported
    } else if last_usage.is_object() {
        LastUsage::Reported(codex_tokens(last_usage, &mut p.diagnostics).map(Box::new))
    } else {
        p.diagnostics.push("missing_or_invalid_token_field");
        LastUsage::Reported(None)
    };
    p.event = Some(Event::Usage {
        key: label(&payload["response_id"])
            .or_else(|| label(&payload["request_id"]))
            .or_else(|| label(&info["request_id"])),
        native_session: None,
        model: label(&payload["model"]),
        tokens: codex_tokens(u, &mut p.diagnostics),
        cumulative,
        input: input_evidence(&Provider::Codex, u),
        last,
    });
}

/// Validate input separately: output and duration partitions are never numerator terms.
fn input_evidence(provider: &Provider, u: &Value) -> InputEvidence {
    let mut evidence = InputEvidence::default();
    let fields: &[&str] = if *provider == Provider::Claude {
        &[
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
    } else {
        &[
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
        ]
    };
    // Codex CLI `token_count` records do not emit `cache_write_input_tokens`
    // (see `codex_tokens`, which reports `cache_write_counter_unreported`), and
    // the Codex numerator is `input_tokens` alone. The cache-write field is
    // therefore optional for Codex: absent means zero, present must be valid
    // and still takes part in the subset check below.
    let optional_from = if *provider == Provider::Codex {
        2
    } else {
        fields.len()
    };
    for (i, field) in fields.iter().enumerate() {
        let value = &u[*field];
        if value.is_null() {
            if i < optional_from {
                evidence.reasons.insert("missing_input_fields");
            }
        } else if value.as_u64().is_none() {
            evidence.reasons.insert("invalid_input_fields");
        }
    }
    if !fields.iter().any(|field| u.get(*field).is_some())
        && u.as_object().is_some_and(|o| !o.is_empty())
        && u.get("output_tokens").is_none()
        && u.get("reasoning_output_tokens").is_none()
    {
        evidence.reasons.insert("unsupported_schema");
    }
    if evidence.reasons.is_empty() {
        let input = u[fields[0]].as_u64().unwrap();
        let read = u[fields[1]].as_u64().unwrap();
        let write = if u[fields[2]].is_null() {
            0
        } else {
            u[fields[2]].as_u64().unwrap()
        };
        if *provider == Provider::Codex {
            match read.checked_add(write) {
                None => {
                    evidence.reasons.insert("arithmetic_overflow");
                }
                Some(subsets) if subsets > input => {
                    evidence.reasons.insert("overlapping_input_subsets");
                }
                _ => {}
            }
        } else if input
            .checked_add(read)
            .and_then(|n| n.checked_add(write))
            .is_none()
        {
            evidence.reasons.insert("arithmetic_overflow");
        }
    }
    if *provider == Provider::Claude {
        let parts = &u["cache_creation"];
        if let (Some(five), Some(hour), Some(write)) = (
            parts["ephemeral_5m_input_tokens"].as_u64(),
            parts["ephemeral_1h_input_tokens"].as_u64(),
            u["cache_creation_input_tokens"].as_u64(),
        ) {
            match five.checked_add(hour) {
                None => {
                    evidence.reasons.insert("arithmetic_overflow");
                }
                Some(partitions) if partitions > write => {
                    evidence.reasons.insert("overlapping_input_subsets");
                }
                _ => {}
            }
        }
    }
    evidence
}
