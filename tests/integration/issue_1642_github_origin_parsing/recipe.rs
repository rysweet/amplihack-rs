//! Group (d): `workflow-terminal-state.yaml` step `probe-terminal-state`
//! sources the shared helper and rejects hostile origins and PR URLs. Helper
//! lookup is covered in `recipe_lookup.rs`.

use super::fixture::{Fixture, Outcome, helper_path, tool_path, workspace_root};
use std::fs;
use std::process::Command;

pub(super) fn probe_command() -> String {
    let path = workspace_root().join("amplifier-bundle/recipes/workflow-terminal-state.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let recipe: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse recipe");
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|step| step["id"] == "probe-terminal-state")
        .and_then(|step| step["command"].as_str())
        .expect("probe-terminal-state command")
        .to_owned()
}

/// The recipe's own `parse_github_pr_url` function body, verbatim.
fn pr_url_function() -> String {
    let command = probe_command();
    let lines: Vec<&str> = command.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "parse_github_pr_url() {")
        .expect("recipe must define parse_github_pr_url");
    let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];
    let end = lines[start..]
        .iter()
        .position(|line| *line == format!("{indent}}}"))
        .expect("end of parse_github_pr_url")
        + start;
    lines[start..=end].join("\n")
}

fn parse_pr_url(url: &str) -> Outcome {
    let output = Command::new("bash")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .arg("-c")
        .arg(r#". "$1"; eval "$2"; parse_github_pr_url "$3""#)
        .arg("pr-url")
        .arg(helper_path())
        .arg(pr_url_function())
        .arg(url)
        .output()
        .expect("run bash");
    Outcome::from(output)
}

#[test]
fn recipe_pr_url_parser_accepts_real_pr_urls() {
    for url in [
        "https://github.com/o/r/pull/5",
        "http://github.com/o/r/pull/5",
        "https://tok@github.com/o/r/pull/5",
        concat!("https://o:", "x@github.com/o/r/pull/5"),
        "https://github.com/o/r/pull/5?notification_referrer_id=1",
        "https://github.com/o/r/pull/5#discussion_r1",
        // One trailing `/`, as the shared parser allows.
        "https://github.com/o/r/pull/5/",
    ] {
        let out = parse_pr_url(url);
        assert!(
            out.success() && out.stdout == "o/r/5\n",
            "{url} -> o/r/5\n{}",
            out.describe()
        );
    }
}

#[test]
fn recipe_pr_url_parser_rejects_hostile_and_malformed_pr_urls() {
    let mut failures = Vec::new();
    for url in [
        "https://evil.example#@github.com/o/r/pull/5",
        "https://evil.example?@github.com/o/r/pull/5",
        "https://evil.example/x@github.com/o/r/pull/5",
        r"https://evil.example\@github.com/o/r/pull/5",
        "https://a@github.com/x@github.com/o/r/pull/5",
        "https://github.com/o/r/pull/0",
        "https://github.com/o/r/pull/007",
        "https://github.com/o/r/pull/12345678901",
        "https://github.com/o/r/pull/5x",
        "https://github.com/o/r/pull/",
        "https://github.com/o/r/pull/5//",
        "https://github.com/o/r/issues/5",
        "https://github.com/o/r",
        "ssh://git@github.com/o/r/pull/5",
        "git@github.com:o/r/pull/5",
        "https://gitlab.com/o/r/pull/5",
    ] {
        let out = parse_pr_url(url);
        if out.success() || !out.stdout.is_empty() {
            failures.push(format!("{url:?} must be rejected\n{}", out.describe()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The probe step run against the fixture, on `feat/x` against `main`.
pub(super) fn run_probe(fx: &Fixture, pr_url: &str, envs: &[(&str, &std::ffi::OsStr)]) -> Outcome {
    let mut cmd = fx.command("bash");
    cmd.arg("-c")
        .arg(probe_command())
        .env("REPO_PATH", &fx.repo)
        // The probe sources recipe_var.sh and runs worktree_setup_path.sh from
        // the installed bundle (#1561); a test's own env may override this.
        .env("AMPLIHACK_HOME", workspace_root())
        // #1561: the probe reads its inputs only as RECIPE_VAR_<key>.
        .env("RECIPE_VAR_repo_path", &fx.repo)
        .env("RECIPE_VAR_branch_name", super::fixture::BRANCH)
        .env("RECIPE_VAR_base_ref", "main")
        .env("RECIPE_VAR_pr_number", "")
        .env("RECIPE_VAR_pr_url", pr_url)
        .env("RECIPE_VAR_goal_already_met", "false")
        .env(
            "WORKFLOW_PR_SCOPE_HELPER",
            tool_path("workflow_pr_scope.sh"),
        );
    for (k, v) in envs {
        cmd.env(k, v);
    }
    fx.run(cmd)
}

#[test]
fn probe_rejects_hostile_origins_without_calling_gh() {
    let helper = helper_path();
    let root = workspace_root();
    let mut failures = Vec::new();
    for origin in super::fixture::HOSTILE_ORIGINS {
        // Once through the explicit override, once through the AMPLIHACK_HOME rung.
        for envs in [
            vec![("WORKFLOW_GITHUB_REPO_HELPER", helper.as_os_str())],
            vec![("AMPLIHACK_HOME", root.as_os_str())],
        ] {
            let fx = Fixture::with_origin(origin);
            let out = run_probe(&fx, "", &envs);
            let calls = fx.repo_gh_calls();
            if !calls.is_empty() || out.json().is_none() || out.stderr.contains("evil.example") {
                failures.push(format!(
                    "{origin:?} via {:?}: gh calls {calls:?}\n{}",
                    envs[0].0,
                    out.describe()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

pub(super) const NON_GITHUB_PR_URL: &str = "non-GitHub PR URL supplied";

#[test]
fn probe_does_not_take_a_hostile_pr_url_as_its_target() {
    let helper = helper_path();
    let mut failures = Vec::new();
    for pr_url in [
        "https://evil.example#@github.com/o/r/pull/5",
        "https://evil.example?@github.com/o/r/pull/5",
        "https://evil.example/x@github.com/o/r/pull/5",
        r"https://evil.example\@github.com/o/r/pull/5",
        // D2: the first @github.com/ ends the userinfo, so the owner is
        // x@github.com and the identity check must reject the URL before the
        // recipe's own `${rest#*@github.com/}` strip could read o/r.
        "https://a@github.com/x@github.com/o/r/pull/5",
        "https://github.com/o/r/pull/0",
        "https://github.com/o/r/pull/007",
        "https://github.com/o/r/pull/12345678901",
    ] {
        let fx = Fixture::with_origin("https://github.com/o/r.git");
        let out = run_probe(
            &fx,
            pr_url,
            &[("WORKFLOW_GITHUB_REPO_HELPER", helper.as_os_str())],
        );
        // gh may still list PRs for o/r (the origin is real); it must never be
        // pointed at the hostile host.
        let hostile_call = fx.all_gh_calls().contains("evil.example");
        if !out.stderr.contains(NON_GITHUB_PR_URL) || hostile_call {
            failures.push(format!(
                "{pr_url:?}\ngh calls:\n{}\n{}",
                fx.all_gh_calls(),
                out.describe()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn probe_still_takes_credential_bearing_github_pr_urls() {
    let helper = helper_path();
    for pr_url in [
        "https://tok@github.com/o/r/pull/5",
        concat!("https://o:", "x@github.com/o/r/pull/5"),
    ] {
        let fx = Fixture::with_origin("https://github.com/o/r.git");
        let out = run_probe(
            &fx,
            pr_url,
            &[("WORKFLOW_GITHUB_REPO_HELPER", helper.as_os_str())],
        );
        assert!(
            !out.stderr.contains(NON_GITHUB_PR_URL),
            "{pr_url} is a github.com PR URL\n{}",
            out.describe()
        );
    }
}
