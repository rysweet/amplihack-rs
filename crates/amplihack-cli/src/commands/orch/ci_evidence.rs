//! Mechanical QA entry check for CI evidence (#2342).
//!
//! Preparation agents observe CI and wait for the run on the current head to
//! finish; this module only checks the completed record they hand to QA. It
//! does not observe GitHub, wait, judge policy or establish merge readiness:
//! the merge gate reads GitHub's own check state for the merged head.
//!
//! Checks run in this order, and the first that fails is reported:
//!
//! 1. the input parses as a JSON object, else "invalid CI evidence";
//! 2. `--head` and `head_sha` are each exactly 40 or 64 hexadecimal
//!    characters in either case, with no trimming, prefix or abbreviation,
//!    else "not a complete Git object ID";
//! 3. `head_sha` equals `--head` after lower-casing both, else "belongs to
//!    another head";
//! 4. `ci_total`, `ci_pending` and `ci_failing` are each a JSON number or an
//!    unsigned decimal string that fits in `u64`, else "invalid CI evidence";
//! 5. the checked sum `ci_pending + ci_failing` does not exceed `ci_total`,
//!    else "inconsistent counts";
//! 6. `ci_pending` is zero, else "CI is still pending";
//! 7. `ci_failing` is zero, else "CI contains failures";
//! 8. `ci_status` is exactly `GREEN`, else "CI has not completed green";
//! 9. `ci_total` is above zero, else "no completed checks observed".
//!
//! Pending comes before failures on purpose: a run still in progress means
//! wait for it, and a record that is both pending and failing must not be
//! read as a final failure. Messages name the field at fault and never
//! repeat any value from the record.

use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Map, Value};
use std::io::Read;

/// Largest CI record, in bytes, that `ci-evidence-ready` reads from stdin.
pub(super) const STDIN_MAX: u64 = 64 * 1024;

/// Read the CI record from `reader`, refusing anything over [`STDIN_MAX`].
pub(super) fn read_capped(reader: impl Read) -> Result<String> {
    let mut buf = Vec::new();
    reader
        .take(STDIN_MAX + 1)
        .read_to_end(&mut buf)
        .map_err(|e| anyhow!("invalid CI evidence: could not read stdin ({e})"))?;
    ensure!(
        buf.len() as u64 <= STDIN_MAX,
        "invalid CI evidence: input is larger than 64 KiB"
    );
    String::from_utf8(buf).map_err(|_| anyhow!("invalid CI evidence: input is not UTF-8"))
}

/// Validate one extracted CI record before QA executes repository code on
/// `head`, following the check order in the module documentation.
pub(super) fn validate(input: &str, head: &str) -> Result<()> {
    // The parser's own message can quote input, so only its position is kept.
    let value: Value = serde_json::from_str(input).map_err(|e| {
        anyhow!(
            "invalid CI evidence: not valid JSON (line {}, column {})",
            e.line(),
            e.column()
        )
    })?;
    let Value::Object(ci) = value else {
        bail!("invalid CI evidence: not a JSON object");
    };

    ensure!(
        is_object_id(head),
        "QA head (--head) is not a complete Git object ID"
    );
    let evidence_head = string_field(&ci, "head_sha")?;
    ensure!(
        is_object_id(evidence_head),
        "CI evidence head_sha is not a complete Git object ID"
    );
    ensure!(
        evidence_head.eq_ignore_ascii_case(head),
        "CI evidence belongs to another head"
    );

    let total = count_field(&ci, "ci_total")?;
    let pending = count_field(&ci, "ci_pending")?;
    let failing = count_field(&ci, "ci_failing")?;
    ensure!(
        pending.checked_add(failing).is_some_and(|sum| sum <= total),
        "CI evidence has inconsistent counts: ci_pending plus ci_failing exceeds ci_total"
    );
    ensure!(pending == 0, "CI is still pending");
    ensure!(failing == 0, "CI contains failures");
    ensure!(
        string_field(&ci, "ci_status")? == "GREEN",
        "CI has not completed green"
    );
    ensure!(total > 0, "no completed checks observed");
    Ok(())
}

/// A full SHA-1 (40) or SHA-256 (64) object ID, hexadecimal in either case.
fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn string_field<'a>(ci: &'a Map<String, Value>, field: &str) -> Result<&'a str> {
    match ci.get(field) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => bail!("invalid CI evidence: {field} must be a string"),
        None => bail!("invalid CI evidence: {field} is missing"),
    }
}

/// A count is a JSON number or a string of ASCII digits, and fits in `u64`.
fn count_field(ci: &Map<String, Value>, field: &str) -> Result<u64> {
    let parsed = match ci.get(field) {
        None => bail!("invalid CI evidence: {field} is missing"),
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse().ok()
        }
        Some(_) => None,
    };
    parsed.ok_or_else(|| anyhow!("invalid CI evidence: {field} must be an unsigned whole number"))
}

#[cfg(test)]
mod tests {
    //! One test per message in the documented check order (#2342). Each
    //! builds the record with `json!` and reads the whole error chain.
    use super::validate;
    use serde_json::{Value, json};

    const HEAD: &str = "219fee3daf8960880adf143c504c453f309f91a4";

    fn green() -> Value {
        json!({"ci_status":"GREEN","ci_total":"3","ci_pending":"0","ci_failing":"0","head_sha":HEAD})
    }

    fn with(field: &str, value: Value) -> String {
        let mut ci = green();
        ci[field] = value;
        ci.to_string()
    }

    fn err(input: &str, head: &str) -> String {
        match validate(input, head) {
            Ok(()) => panic!("accepted {input} for {head}"),
            Err(e) => format!("{e:#}"),
        }
    }

    #[test]
    fn green_on_current_head_proceeds() {
        validate(&green().to_string(), HEAD).expect("complete green CI for this head");
    }

    #[test]
    fn sha256_head_proceeds() {
        let head = "a".repeat(64);
        validate(&with("head_sha", json!(head)), &head).expect("a SHA-256 object ID is complete");
    }

    #[test]
    fn uppercase_matching_head_proceeds() {
        let upper = HEAD.to_ascii_uppercase();
        validate(&with("head_sha", json!(upper)), HEAD).expect("evidence head in upper case");
        validate(&green().to_string(), &upper).expect("--head in upper case");
    }

    #[test]
    fn numeric_counts_proceed() {
        let ci =
            json!({"ci_status":"GREEN","ci_total":3,"ci_pending":0,"ci_failing":0,"head_sha":HEAD});
        validate(&ci.to_string(), HEAD).expect("JSON numbers are valid counts");
    }

    #[test]
    fn pending_is_wait_not_failure() {
        let mut ci = green();
        ci["ci_status"] = json!("PENDING");
        ci["ci_pending"] = json!("2");
        let e = err(&ci.to_string(), HEAD);
        assert!(e.contains("CI is still pending"), "{e}");
        assert!(
            !e.contains("failures"),
            "pending must not read as a failure: {e}"
        );
    }

    #[test]
    fn pending_takes_precedence_over_failures() {
        let mut ci = green();
        ci["ci_status"] = json!("RED");
        ci["ci_pending"] = json!("1");
        ci["ci_failing"] = json!("1");
        let e = err(&ci.to_string(), HEAD);
        assert!(e.contains("CI is still pending"), "pending means wait: {e}");
        assert!(!e.contains("failures"), "{e}");
    }

    #[test]
    fn failed_is_not_met() {
        let mut ci = green();
        ci["ci_status"] = json!("RED");
        ci["ci_failing"] = json!("1");
        let e = err(&ci.to_string(), HEAD);
        assert!(e.contains("CI contains failures"), "{e}");
        assert!(
            !e.contains("pending"),
            "a failure must not read as a wait: {e}"
        );
    }

    #[test]
    fn missing_is_not_met() {
        for input in ["", "   ", "not json", "[]", "null"] {
            let e = err(input, HEAD);
            assert!(e.contains("invalid CI evidence"), "{input:?}: {e}");
        }
        let e = err(&with("ci_total", json!("0")), HEAD);
        assert!(e.contains("no completed checks observed"), "{e}");
    }

    #[test]
    fn missing_field_is_invalid_evidence() {
        for field in [
            "ci_status",
            "ci_total",
            "ci_pending",
            "ci_failing",
            "head_sha",
        ] {
            let mut ci = green();
            ci.as_object_mut().unwrap().remove(field);
            assert!(
                validate(&ci.to_string(), HEAD).is_err(),
                "accepted a record without {field}"
            );
        }
    }

    #[test]
    fn other_head_is_rejected() {
        let e = err(&with("head_sha", json!("f".repeat(40))), HEAD);
        assert!(e.contains("belongs to another head"), "{e}");
    }

    #[test]
    fn malformed_head_is_rejected() {
        let bad = [
            HEAD[..39].to_string(),
            HEAD[..12].to_string(),
            format!("{}g", &HEAD[..39]),
            String::new(),
            format!("0x{}", &HEAD[..38]),
            format!(" {HEAD}"),
            format!("{HEAD}\n"),
            "a".repeat(41),
        ];
        for head in &bad {
            // The same malformed value as --head and as the record's head_sha.
            let e = err(&with("head_sha", json!(head)), head);
            assert!(
                e.contains("not a complete Git object ID"),
                "--head {head:?}: {e}"
            );
            let e = err(&with("head_sha", json!(head)), HEAD);
            assert!(
                e.contains("not a complete Git object ID"),
                "head_sha {head:?}: {e}"
            );
        }
    }

    #[test]
    fn negative_or_nonnumeric_count_is_rejected() {
        for field in ["ci_total", "ci_pending", "ci_failing"] {
            for value in [
                json!("-1"),
                json!(-1),
                json!("1.5"),
                json!(1.5),
                json!("abc"),
                json!(""),
                json!(" 1"),
                json!(null),
                json!(true),
                json!("18446744073709551616"),
            ] {
                let input = with(field, value.clone());
                assert!(validate(&input, HEAD).is_err(), "accepted {field}={value}");
            }
        }
    }

    #[test]
    fn inconsistent_counts_are_rejected() {
        let mut ci = green();
        ci["ci_total"] = json!("1");
        ci["ci_pending"] = json!("1");
        ci["ci_failing"] = json!("1");
        let e = err(&ci.to_string(), HEAD);
        assert!(e.contains("inconsistent counts"), "{e}");
        // The sum is checked, so values near u64::MAX cannot wrap into consistency.
        let mut ci = green();
        ci["ci_total"] = json!("1");
        ci["ci_pending"] = json!(u64::MAX.to_string());
        ci["ci_failing"] = json!("2");
        let e = err(&ci.to_string(), HEAD);
        assert!(e.contains("inconsistent counts"), "{e}");
    }

    #[test]
    fn lowercase_status_is_not_green() {
        for status in [
            "green",
            "Green",
            "PENDING",
            "RED",
            "NO_CHECKS",
            "UNREADABLE",
            "",
        ] {
            let e = err(&with("ci_status", json!(status)), HEAD);
            assert!(e.contains("CI has not completed green"), "{status:?}: {e}");
        }
    }

    #[test]
    fn messages_never_echo_the_record() {
        let marker = "INJECTED-REASON-7f3a";
        let mut ci = green();
        ci["reason"] = json!(marker);
        ci["ci_status"] = json!(marker);
        let e = err(&ci.to_string(), HEAD);
        assert!(!e.contains(marker), "{e}");
        let e = err(&format!("{{\"ci_status\":\"{marker}\""), HEAD);
        assert!(!e.contains(marker), "{e}");
    }
}
