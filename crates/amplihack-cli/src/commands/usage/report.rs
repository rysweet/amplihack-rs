//! Informational Markdown; never workflow acceptance or completion evidence.
use super::models::{Report, Tokens};
use std::fmt::Write;

fn row(s: &mut String, label: &str, t: Tokens, attempts: &str) {
    let _ = writeln!(
        s,
        "| {label} | {} | {} ({}/{}/{}) | {} | {} | {} | {attempts} |",
        t.input,
        t.cache_write,
        t.cache_write_5m,
        t.cache_write_1h,
        t.cache_write
            .saturating_sub(t.cache_write_5m.saturating_add(t.cache_write_1h)),
        t.cache_read,
        t.output,
        t.reasoning
    );
}

pub fn summary(r: &Report) -> String {
    let mut s = format!(
        "## Workstream usage: {}\n\nInformational observations only; never acceptance or workflow completion evidence.\n\nRepository: `{}`; branch: `{}`; PRs: {:?}.\n\nCompleteness: **{}** ({} diagnostics). Observed interval: {} to {}.\n\n",
        r.workstream.as_deref().unwrap_or("unattributed"),
        r.repository.as_deref().unwrap_or("unknown"),
        r.branch.as_deref().unwrap_or("unknown"),
        r.prs,
        if r.complete {
            "complete for declared inputs"
        } else {
            "INCOMPLETE"
        },
        r.diagnostics.len(),
        r.first_observed.as_deref().unwrap_or("unknown"),
        r.last_observed.as_deref().unwrap_or("unknown")
    );
    if r.allocation == "shared_non_additive" {
        s.push_str("**Shared non-additive total across these PRs. Do not sum their comments. No per-PR allocation is claimed.**\n\n");
    }
    s.push_str("| Provider/model | New input | Cache write (5m/1h/unknown) | Cache read | Output excluding separate reasoning | Separate reasoning | Observed attempts |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for m in &r.models {
        row(
            &mut s,
            &format!("{:?}/{}", m.provider, m.model),
            m.tokens,
            &m.observed_attempts.to_string(),
        );
    }
    if let Some(total) = r.total {
        row(&mut s, "Observed total (each session once)", total, "—");
    } else {
        s.push_str("\n**Total unknown: no reliably deduplicated observations.**\n");
    }
    if let Some(unattributed) = r.unattributed {
        row(&mut s, "Unattributed observed subset", unattributed, "—");
    }
    if !r.unresolved.is_empty() {
        s.push_str("\n**Unresolved observations are non-additive evidence, excluded from totals because identity/deduplication is unknown:**\n\n");
        for u in &r.unresolved {
            let _ = writeln!(
                s,
                "- {:?}/{}: {:?}; {}.",
                u.provider, u.model, u.tokens, u.reason
            );
        }
    }
    s.push_str("\nSession direct / inclusive token totals (inclusive rows overlap):\n\n");
    for session in &r.sessions {
        let _ = writeln!(
            s,
            "- `{}` run `{}` parent `{}`: {} / {}; {}.",
            session.id,
            session.run_id.as_deref().unwrap_or("unknown"),
            session.parent.as_deref().unwrap_or("none"),
            session
                .direct
                .map(|t| t.total().to_string())
                .unwrap_or_else(|| "unknown".into()),
            session
                .inclusive
                .map(|t| t.total().to_string())
                .unwrap_or_else(|| "unknown".into()),
            if session.complete {
                "declared inputs complete"
            } else {
                "INCOMPLETE"
            }
        );
    }
    let _ = writeln!(
        s,
        "\nAttempts: {}\n\nClaude reasoning, when not separately recorded, remains in output. Cache-write duration columns partition cache-write totals.\n\nAPI list-price estimate: **unknown**; price reference/version: **none**. {}",
        r.attempt_semantics, r.price_assumptions
    );
    if !r.diagnostics.is_empty() {
        s.push_str("\nDiagnostics (manifest ordinals, no paths or raw records):\n\n");
        for d in &r.diagnostics {
            let _ = writeln!(
                s,
                "- {} (session {:?}, file {:?}, line {:?})",
                d.code, d.session, d.file, d.line
            );
        }
    }
    s
}
