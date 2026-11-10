//! #1888 RED contract: Rust policy through its helper adapter, with no live gh.
//! CLI contract: select-merge-method --repository OWNER/REPO --repo-root PATH
//! [--merge-method VALUE]. No production API visibility is required by these tests.
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::symlink;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
}
impl Fixture {
    fn new() -> Self {
        let f = Self {
            tmp: tempfile::tempdir().unwrap(),
        };
        for dir in ["repo/.github", "bin", "home", "config", "profiles", "tmp"] {
            fs::create_dir_all(f.tmp.path().join(dir)).unwrap();
        }
        let gh = f.tmp.path().join("bin/gh");
        crate::support::exec_stub::write_executable(
            &gh,
            r#"#!/bin/bash
printf '%s\n' "$@" >> "$TEST_CALLS"
[ "$#" = 2 ] && [ "$1" = api ] && [ "$2" = repos/owner/project ] || exit 91
cat "$TEST_RESPONSE"
exit "$TEST_API_RC"
"#,
            0o755,
        );
        f
    }
    fn config(&self, text: &str) {
        fs::write(self.tmp.path().join("repo/.github/amplihack.yml"), text).unwrap();
    }
    fn run(&self, explicit: Option<&str>, response: &str, rc: u8, repo: &str) -> Output {
        let root = self.tmp.path();
        fs::write(root.join("response"), response).unwrap();
        let mut cmd = Command::new(crate::support::amplihack_bin::amplihack_bin());
        cmd.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("CLAUDE_CONFIG_DIR", root.join("profiles/claude"))
            .env("CODEX_HOME", root.join("profiles/codex"))
            .env("AMPLIHACK_HOME", root.join("profiles/amplihack"))
            .env("GH_CONFIG_DIR", root.join("profiles/gh"))
            .env("GH_REPO", "owner-b/repo-b")
            .env("TMPDIR", root.join("tmp"))
            .env("TEST_CALLS", root.join("calls"))
            .env("TEST_RESPONSE", root.join("response"))
            .env("TEST_API_RC", rc.to_string())
            .current_dir(root.join("home")) // configuration must use repo-root, not cwd
            .args([
                "orch",
                "helper",
                "select-merge-method",
                "--repository",
                repo,
                "--repo-root",
            ])
            .arg(root.join("repo"));
        if let Some(value) = explicit {
            cmd.args(["--merge-method", value]);
        }
        cmd.output().unwrap()
    }
    fn select(&self, explicit: Option<&str>, allowed: [bool; 3]) -> Output {
        self.run(explicit, &permissions(allowed), 0, "owner/project")
    }
}
fn permissions(a: [bool; 3]) -> String {
    json!({"allow_merge_commit":a[0],"allow_squash_merge":a[1],"allow_rebase_merge":a[2]})
        .to_string()
}
fn success(out: Output, method: &str, source: &str) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // from_slice rejects trailing JSON objects and prose; output is one object.
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(value.is_object());
    assert_eq!(value["method"], method);
    assert_eq!(value["source"], source);
    let rationale = value["rationale"].as_str().expect("string rationale");
    assert!(!rationale.trim().is_empty());
    assert!(
        rationale.contains(method),
        "rationale must explain chosen method: {rationale}"
    );
}
fn rejected(out: Output) {
    assert!(
        !out.status.success(),
        "selection unexpectedly succeeded: {:?}",
        out
    );
    assert!(
        out.stdout.is_empty(),
        "failure must not print a success object"
    );
    assert!(
        !out.stderr.is_empty(),
        "failure must have an actionable diagnostic"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("unrecognized subcommand"),
        "RED: selector command is not implemented yet"
    );
}

#[test]
fn defaults_cover_every_permission_combination() {
    for mask in 0..8 {
        let f = Fixture::new();
        let a = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
        let out = f.select(None, a);
        if mask == 0 {
            rejected(out);
        } else {
            success(
                out,
                if a[1] {
                    "squash"
                } else if a[0] {
                    "merge"
                } else {
                    "rebase"
                },
                "default",
            );
        }
    }
}
#[test]
fn repository_preference_and_explicit_override_are_distinct_sources() {
    let f = Fixture::new();
    f.config("unrelated: {enabled: true}\nmerge_method: merge\n");
    success(f.select(None, [true; 3]), "merge", "repository");
    success(f.select(Some(""), [true; 3]), "merge", "repository");
    success(f.select(Some("rebase"), [true; 3]), "rebase", "explicit");
    // A valid but disallowed lower-priority preference must not defeat override.
    success(
        f.select(Some("squash"), [false, true, false]),
        "squash",
        "explicit",
    );
}
#[test]
fn missing_key_and_empty_context_use_default() {
    let f = Fixture::new();
    f.config("unrelated: true\n");
    success(f.select(Some(""), [true; 3]), "squash", "default");
    success(f.select(None, [true; 3]), "squash", "default");
}
#[test]
fn effective_disallowed_method_never_falls_back() {
    for method in ["merge", "squash", "rebase"] {
        let f = Fixture::new();
        let a = match method {
            "merge" => [false, true, true],
            "squash" => [true, false, true],
            _ => [true, true, false],
        };
        rejected(f.select(Some(method), a));
        f.config(&format!("merge_method: {method}\n"));
        rejected(f.select(None, a));
    }
}
#[test]
fn unsupported_explicit_context_is_rejected_exactly() {
    let f = Fixture::new();
    for value in [
        "MERGE",
        " merge",
        "merge ",
        "auto",
        "--admin",
        "merge\nsquash",
    ] {
        rejected(f.select(Some(value), [true; 3]));
    }
}
#[test]
fn malformed_configuration_is_not_hidden_by_explicit_override() {
    let f = Fixture::new();
    for yaml in [
        "",
        "null",
        "[]",
        "[true,false,false]",
        "merge_method: null",
        "merge_method: ''",
        "merge_method: 1",
        "merge_method: true",
        "merge_method: []",
        "merge_method: {}",
        "merge_method: MERGE",
        "merge_method: ' merge'",
        "merge_method: auto",
        "merge_method: [",
        "merge_method: merge\nmerge_method: squash\n",
        "merge_method: merge\n---\nmerge_method: rebase\n",
    ] {
        f.config(yaml);
        rejected(f.select(None, [true; 3]));
        rejected(f.select(Some("squash"), [true; 3]));
    }
}
#[test]
fn api_failures_and_schema_errors_fail_closed() {
    let f = Fixture::new();
    let refusal = f.run(None, &permissions([true; 3]), 79, "owner/project");
    assert_eq!(refusal.status.code(), Some(79));
    rejected(refusal);
    for body in [
        "",
        "not json",
        "null",
        "[]",
        "[true,false,false]",
        "{}",
        "{}{}",
        r#"{"allow_merge_commit":true,"allow_squash_merge":true}"#,
        r#"{"allow_merge_commit":"true","allow_squash_merge":true,"allow_rebase_merge":true}"#,
        r#"{"allow_merge_commit":true,"allow_squash_merge":null,"allow_rebase_merge":true}"#,
        r#"{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":1}"#,
    ] {
        rejected(f.run(None, body, 0, "owner/project"));
    }
}
#[test]
fn repository_identity_is_validated_and_api_argv_is_separate() {
    let f = Fixture::new();
    for repo in [
        "",
        "owner",
        "owner/project/extra",
        "../project",
        "-owner/project",
        "owner/pro ject",
        "owner/project;echo",
    ] {
        rejected(f.run(None, &permissions([true; 3]), 0, repo));
    }
    assert!(
        !f.tmp.path().join("calls").exists(),
        "invalid identity must not invoke gh"
    );
    success(f.select(None, [true; 3]), "squash", "default");
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("calls")).unwrap(),
        "api\nrepos/owner/project\n"
    );
}
#[test]
fn configuration_symlinks_must_resolve_within_repository() {
    let f = Fixture::new();
    let root = f.tmp.path();
    let config = root.join("repo/.github/amplihack.yml");
    fs::write(root.join("repo/policy.yml"), "merge_method: merge\n").unwrap();
    symlink("../policy.yml", &config).unwrap();
    success(f.select(None, [true; 3]), "merge", "repository");
    fs::remove_file(&config).unwrap();
    fs::write(root.join("outside.yml"), "merge_method: merge\n").unwrap();
    symlink(root.join("outside.yml"), &config).unwrap();
    rejected(f.select(Some("squash"), [true; 3]));
    fs::remove_file(&config).unwrap();
    symlink("missing.yml", &config).unwrap();
    rejected(f.select(None, [true; 3]));
}
#[test]
fn configuration_directory_escape_and_unreadable_file_fail() {
    let f = Fixture::new();
    let root = f.tmp.path();
    fs::remove_dir(root.join("repo/.github")).unwrap();
    fs::create_dir(root.join("outside")).unwrap();
    fs::write(root.join("outside/amplihack.yml"), "merge_method: merge").unwrap();
    symlink(root.join("outside"), root.join("repo/.github")).unwrap();
    rejected(f.select(None, [true; 3]));
    fs::remove_file(root.join("repo/.github")).unwrap();
    fs::create_dir(root.join("repo/.github")).unwrap();
    // A directory at the config path deterministically fails read even as root.
    fs::create_dir(root.join("repo/.github/amplihack.yml")).unwrap();
    rejected(f.select(None, [true; 3]));
}

#[test]
fn recipes_declare_empty_context_and_forward_it_at_nested_boundaries() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/recipes");
    for name in [
        "auto-drive-to-merge",
        "auto-drive-light",
        "autodrive-merge-loop",
    ] {
        let text = fs::read_to_string(root.join(format!("{name}.yaml"))).unwrap();
        let recipe: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(recipe["context"]["repository"].as_str(), Some(""));
        assert_eq!(
            recipe["context"]["merge_method"].as_str(),
            Some(""),
            "{name}: no implicit squash preference"
        );
        for step in recipe["steps"].as_sequence().unwrap() {
            if matches!(
                step["recipe"].as_str(),
                Some("auto-drive-to-merge" | "autodrive-merge-loop")
            ) {
                assert_eq!(
                    step["context"]["repository"].as_str(),
                    Some("{{repository}}")
                );
                assert_eq!(
                    step["context"]["merge_method"].as_str(),
                    Some("{{merge_method}}"),
                    "{name}: nested recipe must receive unchanged context"
                );
            }
        }
        if name == "autodrive-merge-loop" {
            let gate = recipe["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|s| s["id"].as_str() == Some("step-03-merge-gate"))
                .unwrap();
            assert!(
                gate["command"].as_str().unwrap().contains("--merge-method")
                    && gate["command"]
                        .as_str()
                        .unwrap()
                        .contains("--repository \"${RECIPE_VAR_repository:-}\""),
                "merge loop must forward context to the trusted gate"
            );
        }
    }
}

#[test]
fn configuration_fifo_is_rejected_without_reading_it() {
    use std::ffi::CString;
    let f = Fixture::new();
    let path = f.tmp.path().join("repo/.github/amplihack.yml");
    let path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    rejected(f.select(Some("merge"), [true; 3]));
}
