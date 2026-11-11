use super::{evidence, private_state::State, secure_fs};
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

pub const FIELDS: &[&str] = &[
    "loop_name",
    "loop_round_label",
    "loop_history",
    "loop_repo_path",
    "loop_agent_working_dir",
    "loop_baseline_ref",
    "loop_child_exit_code",
    "loop_findings_current",
    "loop_findings_previous",
    "loop_test_signal",
    "loop_test_signal_previous",
    "loop_ci_signal",
    "loop_ci_signal_previous",
    "loop_clean_token",
    "loop_round_verdict",
    "loop_last_round_output",
];

pub fn validate_context(context: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        context.keys().all(|k| FIELDS.contains(&k.as_str())),
        "unknown evaluator context field"
    );
    ensure!(
        context
            .values()
            .all(|v| v.len() <= evidence::LIMIT && !v.contains('\0')),
        "invalid evaluator data"
    );
    Ok(())
}

/// Reads only the output immediately following the last completed enforcer
/// status, with no later step status. Mirrors the runner's table contract.
pub(crate) fn verdict(log: &str) -> &'static str {
    let status =
        regex::Regex::new(r"^  [^ ]+ step-[^ :(]+( \([^)]*\))?: ").expect("constant regex");
    let completed = regex::Regex::new(
        r"^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$",
    )
    .expect("constant regex");
    let mut pending = false;
    let mut result = "STUCK";
    for line in log.lines() {
        if pending {
            let token = line
                .strip_prefix("    Output: LOOP_HEALTH: ")
                .or_else(|| line.strip_prefix("LOOP_HEALTH: "));
            result = match token.and_then(|v| v.split(' ').next()) {
                Some("CONTINUE") => "CONTINUE",
                Some("DONE") => "DONE",
                _ => "STUCK",
            };
            pending = false;
        }
        if status.is_match(line) {
            result = "STUCK";
            pending = completed.is_match(line);
        }
    }
    result
}

pub fn run(
    state: &State,
    record: &serde_json::Value,
    child_rc: i32,
) -> Result<(serde_json::Value, i32)> {
    let mut context = state.context.clone();
    let label = &state.round_label;
    for (key, value) in [
        ("loop_name", "auto-drive:crusty".to_string()),
        ("loop_round_label", label.clone()),
        ("loop_repo_path", state.repo.to_string_lossy().into_owned()),
        (
            "loop_agent_working_dir",
            state.review_dir.to_string_lossy().into_owned(),
        ),
        ("loop_child_exit_code", child_rc.to_string()),
        (
            "loop_round_verdict",
            record["crusty_verdict"]
                .as_str()
                .unwrap_or("MISSING")
                .into(),
        ),
        (
            "loop_findings_current",
            String::from_utf8(secure_fs::read(
                &state.record.with_extension("json.findings"),
                evidence::LIMIT,
            )?)?,
        ),
        (
            "loop_test_signal",
            record["test_signal"].as_str().unwrap_or("").into(),
        ),
        (
            "loop_ci_signal",
            record["ci_signal"].as_str().unwrap_or("").into(),
        ),
    ] {
        context.insert(key.into(), value);
    }
    let history = context.entry("loop_history".into()).or_default();
    history.push_str(&format!(
        "\n{label}: crusty_verdict={} rc={child_rc}",
        record["crusty_verdict"].as_str().unwrap_or("MISSING")
    ));
    let round_log = state.state_dir.join(format!("crusty-{label}.log"));
    if round_log.is_file() {
        let o = Command::new("bash")
            .arg(state.launch_tools.join("autodrive_round_tail.sh"))
            .arg(round_log)
            .output()?;
        ensure!(o.status.success(), "cannot collect round output");
        context.insert(
            "loop_last_round_output".into(),
            String::from_utf8(o.stdout)?,
        );
    }
    let health_log = state.state_dir.join(format!("crusty-{label}-health.log"));
    let mut cmd = Command::new(&state.binary);
    cmd.args(["recipe", "run"])
        .arg(
            state
                .launch_tools
                .join("../recipes/loop-health-evaluator.yaml"),
        )
        .arg("--working-dir")
        .arg(&state.review_dir)
        .current_dir(&state.review_dir);
    // Preserve the launcher's policy ceilings and sibling depth, never values
    // from agent output or context. No shell evaluates any of these arguments.
    for (key, value) in [
        ("AMPLIHACK_SESSION_DEPTH", &state.depth),
        ("AMPLIHACK_MAX_DEPTH", &state.ceiling),
    ] {
        if let Some(value) = value {
            cmd.env(key, value);
        } else {
            cmd.env_remove(key);
        }
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix(".evaluator-context-");
    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));
    let context_dir = builder.tempdir_in(&state.state_dir)?;
    for (key, value) in context {
        if matches!(key.as_str(), "loop_clean_token" | "loop_round_verdict") {
            cmd.arg("-c").arg(format!("{key}={value}"));
        } else {
            let path = context_dir.path().join(&key);
            secure_fs::output(&path)?.write_all(value.as_bytes())?;
            cmd.arg("--context-from-file")
                .arg(format!("{key}={}", path.display()));
        }
    }
    let file = secure_fs::output(&health_log)?;
    let status = cmd.stdout(file.try_clone()?).stderr(file).status()?;
    let log = secure_fs::read(&health_log, evidence::LIMIT)?;
    let log = String::from_utf8_lossy(&log);
    eprint!("{log}");
    ensure!(
        evidence::review_stamp(&state.launch_tools, &state.review_dir)? == state.review_stamp,
        "loop-health evaluator changed the trusted review directory"
    );
    let rc = status.code().unwrap_or(1);
    let (result, code) = classify_exit(rc, &log);
    let evaluator_verdict = result;
    // DONE from convergence evaluation cannot certify an unresolved review.
    // The shared driver retains its re-review/unchanged-DONE protection.
    let result = if result == "DONE" { "CONTINUE" } else { result };
    let result = serde_json::json!({"loop":"crusty","loop_result":result,"evaluator_verdict":evaluator_verdict,"round_label":label,
        "round_verdict":record["crusty_verdict"],"source":"evaluator","evaluator_invoked":true});
    Ok((result, code))
}

fn classify_exit(rc: i32, log: &str) -> (&'static str, i32) {
    let filtered = log_body(log);
    if rc == 79
        || filtered
            .iter()
            .any(|l| l.contains("BLOCKED_TERMINAL orchestration_unavailable"))
    {
        ("TERMINAL_POLICY_REFUSAL", 79)
    } else if rc == 75 {
        ("ORPHANED", 75)
    } else if rc != 0
        && filtered.iter().any(|line| {
            line.strip_prefix("amplihack.recipe.failure_class ")
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .is_some_and(|v| v["class"] == "usage_limit")
        })
    {
        ("USAGE_LIMIT", 1)
    } else if rc != 0 {
        ("STUCK", 1)
    } else {
        let v = verdict(log);
        (v, i32::from(v == "STUCK"))
    }
}

fn log_body(log: &str) -> Vec<&str> {
    let skip = regex::Regex::new(
        r"^\s*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)",
    )
    .expect("constant regex");
    let lines: Vec<_> = log.lines().collect();
    let end = lines
        .windows(2)
        .position(|pair| {
            pair[0].starts_with("Recipe: ")
                && (pair[1].starts_with("Run ID: ") || pair[1].starts_with("Status: "))
        })
        .unwrap_or(lines.len());
    lines[..end]
        .iter()
        .copied()
        .filter(|line| !skip.is_match(line))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_classes_ignore_input_echoes_and_nested_json() {
        let usage = "amplihack.recipe.failure_class {\"class\":\"usage_limit\"}";
        assert_eq!(classify_exit(1, usage), ("USAGE_LIMIT", 1));
        for log in [
            format!("Recipe: review\nStatus: failed\n{usage}"),
            format!("[12:34:56] [amplihack:reviewer] {usage}"),
            format!("  {usage}"),
            "amplihack.recipe.failure_class {\"evidence\":{\"class\":\"usage_limit\"}}".into(),
        ] {
            assert_eq!(classify_exit(1, &log), ("STUCK", 1));
        }
        let marker = "BLOCKED_TERMINAL orchestration_unavailable";
        assert_eq!(classify_exit(0, marker), ("TERMINAL_POLICY_REFUSAL", 79));
        assert_eq!(
            classify_exit(1, &format!("[12:34:56] [amplihack:reviewer] {marker}")),
            ("STUCK", 1)
        );
        assert_eq!(
            classify_exit(1, &format!("Recipe: review\nRun ID: xyz\n{marker}")),
            ("STUCK", 1)
        );
    }
}
