//! Validation of the recovery agent's reply.
//!
//! The reply is untrusted. Checks run in the documented order and the first
//! failure turns the outcome into a validator handoff that keeps the agent's
//! reason. The agent never sets `recurrence` or `source`.

use serde::Deserialize;

use super::record::{Choice, Record, TEXT_LIMIT, check_text, is_unsafe_char};

/// The fields an agent may send.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    choice: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    repair: Option<String>,
    #[serde(default)]
    alternative: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    needs: Option<String>,
}

/// A validated agent decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub choice: Choice,
    pub reason: String,
    pub repair: Option<String>,
    pub alternative: Option<String>,
    pub owner: Option<String>,
    pub needs: Option<String>,
}

/// Why a reply was refused, plus whatever reason the agent gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub check: String,
    pub agent_reason: Option<String>,
}

pub struct Context<'a> {
    pub allowed: &'a [Choice],
    pub alternatives: &'a [String],
    pub trusted_owners: &'a [String],
    pub recurrence: bool,
    /// Earlier records of this step (for check 7).
    pub prior: &'a [Record],
    pub signature: &'a str,
}

/// The `{"step_recovery":…}` object on the final non-empty line of the
/// agent's text. An object on any earlier line (a quote of the evidence, or
/// a line the failed command planted) is never the reply.
pub fn find_reply(text: &str) -> Option<serde_json::Value> {
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?;
    crate::commands::orch::extract_json_with_field(last, "step_recovery")
        .map(|v| v["step_recovery"].clone())
}

pub(super) fn refuse(check: impl Into<String>, agent_reason: Option<String>) -> Refusal {
    Refusal {
        check: check.into(),
        agent_reason,
    }
}

/// Checks 1-7 of the reference page, in order.
pub fn validate(reply: Option<serde_json::Value>, ctx: &Context<'_>) -> Result<Decision, Refusal> {
    // 1. A reply exists and parses.
    let Some(value) = reply else {
        return Err(refuse(
            "check 1: no step_recovery reply line was found",
            None,
        ));
    };
    let loose_reason = value
        .get("reason")
        .and_then(|r| r.as_str())
        .map(str::to_string);
    // 2. No unknown fields (and every field has the right type).
    let reply: Reply = serde_json::from_value(value)
        .map_err(|e| refuse(format!("check 2: reply is malformed ({e})"), loose_reason))?;
    let reason = reply.reason.clone();
    // 3. The choice is allowed now.
    let choice = Choice::parse(&reply.choice)
        .filter(|c| ctx.allowed.contains(c))
        .ok_or_else(|| {
            refuse(
                format!(
                    "check 3: choice {:?} is not in allowed_choices",
                    reply.choice
                ),
                reason.clone(),
            )
        })?;
    // 4. Text fields are within limits and free of control characters.
    let texts = [
        ("reason", &reply.reason),
        ("repair", &reply.repair),
        ("alternative", &reply.alternative),
        ("owner", &reply.owner),
        ("needs", &reply.needs),
    ];
    for (name, value) in texts {
        if let Some(v) = value {
            check_text(name, v).map_err(|e| refuse(format!("check 4: {e}"), reason.clone()))?;
        }
    }
    // 5. Required fields present, and no others.
    let required = [
        ("reason", reply.reason.is_some(), true),
        (
            "repair",
            reply.repair.is_some(),
            choice == Choice::RetryAfterRepair,
        ),
        (
            "alternative",
            reply.alternative.is_some(),
            choice == Choice::AlternativeStep,
        ),
        ("owner", reply.owner.is_some(), choice == Choice::Handoff),
        ("needs", reply.needs.is_some(), choice == Choice::Handoff),
    ];
    for (name, present, wanted) in required {
        if present != wanted {
            let what = if wanted {
                "is required"
            } else {
                "is not allowed"
            };
            return Err(refuse(
                format!("check 5: {name} {what} with {}", choice.as_str()),
                reason.clone(),
            ));
        }
    }
    // 6. Declared alternative, trusted owner.
    if let Some(alt) = &reply.alternative
        && !ctx.alternatives.contains(alt)
    {
        return Err(refuse(
            format!("check 6: alternative {alt:?} was not declared"),
            reason.clone(),
        ));
    }
    if let Some(owner) = &reply.owner
        && !ctx.trusted_owners.contains(owner)
    {
        return Err(refuse(
            "check 6: owner is not a trusted value",
            reason.clone(),
        ));
    }
    // 7. On a recurrence, the same alternative is not chosen again.
    if ctx.recurrence
        && let Some(alt) = &reply.alternative
        && ctx
            .prior
            .iter()
            .any(|r| r.failure.signature == ctx.signature && r.alternative.as_deref() == Some(alt))
    {
        return Err(refuse(
            format!("check 7: alternative {alt:?} was already chosen for this failure"),
            reason.clone(),
        ));
    }
    Ok(Decision {
        choice,
        reason: reason.unwrap_or_default(),
        repair: reply.repair,
        alternative: reply.alternative,
        owner: reply.owner,
        needs: reply.needs,
    })
}

/// Cuts text to at most `TEXT_LIMIT` bytes on a character boundary.
pub fn clip(text: &str) -> String {
    let mut end = text.len().min(TEXT_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// The reason of a validator handoff: the failed check, then the agent's
/// own reason, with control characters removed, cut to 1024 bytes.
pub fn refusal_reason(refusal: &Refusal) -> String {
    let mut text = format!("validator: {}", refusal.check);
    if let Some(agent) = refusal.agent_reason.as_deref().map(str::trim)
        && !agent.is_empty()
    {
        text.push_str("; agent reason: ");
        text.push_str(agent);
    }
    let cleaned: String = text
        .chars()
        .map(|c| if is_unsafe_char(c) { ' ' } else { c })
        .collect();
    clip(&cleaned)
}
