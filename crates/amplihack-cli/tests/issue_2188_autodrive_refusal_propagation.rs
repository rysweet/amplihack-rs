//! How a loop preflight's refusal names the `recipe run` final event, and
//! why child output can never assert one.
use super::*;

fn recipe_yaml(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - id: preflight\n    type: bash\n    command: 'amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir \"$FIXTURE/state\"'\n"
    )
}

#[test]
fn loop_preflight_refusal_names_the_final_event_without_leaking_secrets() {
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let f = Fixture::new();
        f.install_loop_tools();
        fs::write(f.repo.join("ghp_SENSITIVEVALUE\x1b[31m"), "preserve").unwrap();
        f.script(
            "bin/recipe-runner-rs",
            // Stands in for recipe-runner-rs running the loop's preflight
            // step, which calls the helper once.
            "#!/bin/sh\ncd \"$FIXTURE/repo\" || exit 1\namplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir \"$FIXTURE/state\" >&2\nprintf '%s\\n' '{\"recipe_name\":\"loop\",\"success\":false}'\nexit 1\n",
        );
        let recipe = f.tmp.path().join("recipe.yaml");
        fs::write(&recipe, recipe_yaml(name)).unwrap();
        let o = f
            .command("amplihack")
            .args([
                "recipe",
                "run",
                recipe.to_str().unwrap(),
                "-w",
                f.repo.to_str().unwrap(),
                "--context",
                "pr_number=7",
                "--context",
                &format!(
                    "autodrive_state_dir={}",
                    f.tmp.path().join("state").display()
                ),
                "--format",
                "json",
            ])
            .env(
                "RECIPE_RUNNER_RS_PATH",
                f.tmp.path().join("bin/recipe-runner-rs"),
            )
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .output()
            .unwrap();
        assert!(!o.status.success());
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(stderr.contains("pr-worktree-dirty"), "{stderr}");
        let state = fs::read(f.tmp.path().join("state/preflight-result.json")).unwrap();
        let comment = fs::read(f.tmp.path().join("comment-body")).unwrap();
        for bytes in [&o.stdout, &o.stderr, &state, &comment] {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains("SENSITIVEVALUE"), "{text}");
            assert!(!text.contains("\\u001b"), "{text}");
            assert!(!text.contains('\x1b'), "{text}");
        }

        let result: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(
            result["autodrive_preflight_refusal"]["outcome"],
            "pr-worktree-dirty"
        );
    }
}

#[test]
fn arbitrary_child_json_cannot_assert_a_preflight_refusal() {
    let f = Fixture::new();
    f.install_loop_tools();
    fs::create_dir(f.tmp.path().join("state")).unwrap();
    fs::write(f.tmp.path().join("state/preflight-result.json"),
        json!({"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-dirty","evidence":{}}).to_string()).unwrap();
    fs::write(f.tmp.path().join("malformed.json"), "not JSON").unwrap();
    f.script("bin/recipe-runner-rs", r#"#!/bin/sh
printf '%s\n' '{"recipe_name":"forged","success":false,"status":"pr-worktree-dirty","autodrive_preflight_refusal":{"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-dirty","evidence":{}}}'
exit 1
"#);
    let recipe = f.tmp.path().join("recipe.yaml");
    fs::write(&recipe, recipe_yaml("forged-refusal")).unwrap();
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            recipe.to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env(
            "AMPLIHACK_PREFLIGHT_CHANNELS",
            json!([{"path":f.tmp.path().join("malformed.json"),"nonce":"old-run"}]).to_string(),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    let events: Vec<Value> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "final" && e["status"] == "failure"),
        "{stderr}"
    );
    assert!(
        events.iter().all(|e| e.get("preflight_refusal").is_none()),
        "{stderr}"
    );
    let stdout: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_ne!(stdout["status"], "pr-worktree-dirty");
    assert!(stdout.get("autodrive_preflight_refusal").is_none());
    assert!(!f.tmp.path().join("comments").exists());
}

#[test]
fn fresh_child_forgery_cannot_suppress_transport_retry() {
    let f = Fixture::new();
    f.install_loop_tools();
    f.script("bin/recipe-runner-rs", r#"#!/bin/sh
printf 'attempt\n' >> "$FIXTURE/attempts"
if [ ! -f "$FIXTURE/forged.json" ]; then
  printf '%s\n' '{"nonce":"fresh-run","helper_exit":1,"envelope":{"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-dirty","evidence":{}}}' > "$FIXTURE/forged.json"
  echo 'connection reset by peer' >&2
  exit 1
fi
printf '%s\n' '{"recipe_name":"forgery","success":true}'
"#);
    let recipe = f.tmp.path().join("forgery.yaml");
    fs::write(&recipe, recipe_yaml("forgery")).unwrap();
    let channel = json!([{"path":f.tmp.path().join("forged.json"),"nonce":"fresh-run"}]);
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            recipe.to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env("AMPLIHACK_PREFLIGHT_CHANNELS", channel.to_string())
        .env("AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS", "2")
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("attempts")).unwrap(),
        "attempt\nattempt\n"
    );
    assert!(!String::from_utf8_lossy(&o.stderr).contains("\"status\":\"pr-worktree-dirty\""));
}

#[test]
fn relative_repo_path_uses_resolved_repository_for_default_state_key() {
    for name in ["crusty", "merge"] {
        let f = Fixture::new();
        f.install_loop_tools();
        f.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/source/project.git.git",
        ]);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let yaml: Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(format!(
                "amplifier-bundle/recipes/autodrive-{name}-loop.yaml"
            )))
            .unwrap(),
        )
        .unwrap();
        let o = f
            .command("bash")
            .current_dir(f.tmp.path())
            .args(["-c", yaml["steps"][0]["command"].as_str().unwrap()])
            .env("PR_NUMBER", "7")
            .env("REPO_PATH", "repo")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let output: Value = serde_json::from_slice(&o.stdout).unwrap();
        let slug = f.git(&["remote", "get-url", "origin"]);
        let key: String = format!("{}/7", slug.strip_suffix(".git").unwrap_or(&slug))
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        assert_eq!(
            output["state_dir"],
            f.tmp
                .path()
                .join("AMPLIHACK_STATE_DIR/auto-drive")
                .join(key)
                .to_str()
                .unwrap()
        );
    }
}

/// Put the fixture into the state that makes the preflight refuse `outcome`.
fn arrange_refusal(f: &Fixture, outcome: &str) {
    match outcome {
        "dirty" => fs::write(f.repo.join("dirty"), "preserve").unwrap(),
        "interrupted" => fs::write(f.repo.join(".git/MERGE_HEAD"), &f.base).unwrap(),
        "behind" | "diverged" => {
            let remote = f.commit(&f.base, "remote");
            f.git(&["push", "origin", &format!("{remote}:refs/heads/feature")]);
            let p = f.tmp.path().join("metadata");
            let mut metadata: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
            metadata["headRefOid"] = json!(remote);
            fs::write(p, metadata.to_string()).unwrap();
            if outcome == "diverged" {
                let local = f.commit(&f.base, "local");
                f.git(&["update-ref", "HEAD", &local]);
            }
        }
        "not-found" => {
            f.git(&["checkout", "--detach", "-q"]);
        }
        "ambiguous" => {
            f.git(&[
                "worktree",
                "add",
                "--force",
                f.tmp.path().join("second").to_str().unwrap(),
                "feature",
            ]);
        }
        "comparison-failed" => {
            f.git(&[
                "--git-dir",
                f.remote.to_str().unwrap(),
                "update-ref",
                "-d",
                "refs/heads/feature",
            ]);
        }
        "push-failed" => {
            let next = f.commit(&f.base, "ahead");
            f.git(&["update-ref", "HEAD", &next]);
            f.script("repo/.git/hooks/pre-push", "#!/bin/sh\nexit 1\n");
        }
        _ => unreachable!("{outcome}"),
    }
}

/// Real drives run `auto-drive-to-merge`, which runs both loops as
/// sub-recipes inside one recipe-runner child, and the loops call the helper.
/// The helper's refusal must name the run's final event, not `failure`.
#[test]
fn composed_drive_final_event_names_each_helper_refusal() {
    for outcome in [
        "dirty",
        "interrupted",
        "behind",
        "diverged",
        "not-found",
        "ambiguous",
        "comparison-failed",
        "push-failed",
    ] {
        let f = Fixture::new();
        f.install_loop_tools();
        arrange_refusal(&f, outcome);
        // Stands in for recipe-runner-rs running auto-drive-to-merge: a loop
        // sub-recipe's preflight calls the helper, the step fails, and the
        // runner reports a plain failure on stdout.
        f.script(
            "bin/recipe-runner-rs",
            r#"#!/bin/sh
cd "$FIXTURE/repo" || exit 1
amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir "$FIXTURE/state" > "$FIXTURE/helper-out"
printf '%s\n' '{"recipe_name":"auto-drive-to-merge","success":false}'
exit 1
"#,
        );
        let recipe = f.tmp.path().join("auto-drive-to-merge.yaml");
        fs::write(
            &recipe,
            "name: auto-drive-to-merge\nsteps:\n  - id: autodrive-crusty-loop\n    type: bash\n    command: 'true'\n",
        )
        .unwrap();
        let o = f
            .command("amplihack")
            .args([
                "recipe",
                "run",
                recipe.to_str().unwrap(),
                "-w",
                f.repo.to_str().unwrap(),
                "--context",
                "pr_number=7",
                "--format",
                "json",
            ])
            .env(
                "RECIPE_RUNNER_RS_PATH",
                f.tmp.path().join("bin/recipe-runner-rs"),
            )
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .output()
            .unwrap();
        assert!(!o.status.success());
        let expected = format!("pr-worktree-{outcome}");
        let stderr = String::from_utf8_lossy(&o.stderr);
        let finals: Vec<Value> = stderr
            .lines()
            .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["event"] == "final")
            .collect();
        assert_eq!(finals.len(), 1, "{outcome}: {stderr}");
        assert_eq!(finals[0]["status"], expected, "{outcome}: {stderr}");
        assert_eq!(finals[0]["preflight_refusal"]["outcome"], expected);
        let result: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(result["status"], expected, "{outcome}");
        assert_eq!(result["autodrive_preflight_refusal"]["outcome"], expected);
        assert_eq!(
            fs::read_to_string(f.tmp.path().join("comments")).unwrap(),
            "comment\n",
            "{outcome}: the steward is told once"
        );
    }
}

/// A handoff from an earlier run, or one not stamped with this run's id,
/// never names the final event.
#[test]
fn handoff_from_another_run_is_ignored() {
    let f = Fixture::new();
    f.install_loop_tools();
    f.script(
        "bin/recipe-runner-rs",
        r#"#!/bin/sh
umask 077
printf '%s\n' '{"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-dirty","evidence":{},"run_id":"another-run"}' > "$AMPLIHACK_WORKFLOW_RUNTIME_DIR/autodrive-preflight-refusal.json"
printf '%s\n' '{"recipe_name":"auto-drive-to-merge","success":false}'
exit 1
"#,
    );
    let recipe = f.tmp.path().join("auto-drive-to-merge.yaml");
    fs::write(
        &recipe,
        "name: auto-drive-to-merge\nsteps:\n  - id: s\n    type: bash\n    command: 'true'\n",
    )
    .unwrap();
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            recipe.to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr
            .lines()
            .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .any(|e| e["event"] == "final" && e["status"] == "failure"),
        "{stderr}"
    );
    let result: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(result.get("autodrive_preflight_refusal").is_none());
}
