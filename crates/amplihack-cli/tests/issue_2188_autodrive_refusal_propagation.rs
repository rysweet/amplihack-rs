//! Runner-owned refusal authority and untrusted child evidence regressions.
use super::*;

fn recipe_yaml(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - id: preflight\n    type: bash\n    command: 'amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir \"$FIXTURE/state\"'\n"
    )
}

#[test]
fn runner_owned_refusal_stops_both_loops_before_child_execution() {
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let f = Fixture::new();
        f.install_loop_tools();
        fs::write(f.repo.join("ghp_SENSITIVEVALUE\x1b[31m"), "preserve").unwrap();
        f.script(
            "bin/recipe-runner-rs",
            "#!/bin/sh\nprintf invoked > \"$FIXTURE/attempts\"\nexit 1\n",
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
        assert!(!f.tmp.path().join("attempts").exists());
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
#[cfg(target_os = "linux")]
fn ready_decision_is_handed_to_both_recipes_without_a_second_inspection() {
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let f = Fixture::new();
        f.install_loop_tools();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let recipe_path = root.join(format!("amplifier-bundle/recipes/{name}.yaml"));
        let recipe: Value =
            serde_yaml::from_str(&fs::read_to_string(&recipe_path).unwrap()).unwrap();
        let body = recipe["steps"][0]["command"].as_str().unwrap();
        assert!(body.contains("AMPLIHACK_AUTODRIVE_READY"));
        fs::write(f.tmp.path().join("preflight.sh"), body).unwrap();
        // Exercise the real preflight command without invoking the Linux-only
        // merge gate, which is outside this decision handoff contract.
        let recipe_path = f.tmp.path().join("handoff.yaml");
        fs::write(
            &recipe_path,
            serde_yaml::to_string(
                &json!({"name":name,"steps":[{"id":"preflight","type":"bash","command":body}]}),
            )
            .unwrap(),
        )
        .unwrap();
        f.script(
            "bin/recipe-runner-rs",
            r#"#!/bin/sh
printf '%s' "$AMPLIHACK_AUTODRIVE_READY" > "$FIXTURE/ready"
# A change here formerly caused a second helper refusal, erased by the parent.
printf changed > "$FIXTURE/repo/change-after-inspection"
export REPO_PATH="$FIXTURE/repo" PR_NUMBER=7 RECIPE_VAR_autodrive_pr_input=7
export AUTODRIVE_STATE_DIR="$FIXTURE/state"
bash "$FIXTURE/preflight.sh" > "$FIXTURE/recipe-preflight" || exit 1
printf '%s\n' '{"recipe_name":"handoff","success":true}'
"#,
        );
        let o = f
            .command("amplihack")
            .args([
                "recipe",
                "run",
                recipe_path.to_str().unwrap(),
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
            .env("AMPLIHACK_HOME", &root)
            .env("AMPLIHACK_AUTODRIVE_READY", "untrusted inherited value")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let ready: Value =
            serde_json::from_slice(&fs::read(f.tmp.path().join("ready")).unwrap()).unwrap();
        assert_eq!(ready["outcome"], "ready");
        let preflight: Value =
            serde_json::from_slice(&fs::read(f.tmp.path().join("recipe-preflight")).unwrap())
                .unwrap();
        assert_eq!(preflight["repo"], f.repo.to_str().unwrap());
        assert!(!f.tmp.path().join("comments").exists());
    }
}

#[test]
fn both_recipe_consumers_use_the_ready_envelope_without_reinspection() {
    for name in ["crusty", "merge"] {
        let f = Fixture::new();
        f.install_loop_tools();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let yaml: Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(format!(
                "amplifier-bundle/recipes/autodrive-{name}-loop.yaml"
            )))
            .unwrap(),
        )
        .unwrap();
        let body = yaml["steps"][0]["command"].as_str().unwrap();
        assert!(body.contains("AMPLIHACK_AUTODRIVE_READY"));
        let ready = f.run();
        assert!(ready.status.success());
        fs::write(f.repo.join("change-after-inspection"), "preserve").unwrap();
        let o = f
            .command("bash")
            .args(["-c", body])
            .env("PR_NUMBER", "7")
            .env("REPO_PATH", &f.repo)
            .env("AUTODRIVE_STATE_DIR", f.tmp.path().join("state"))
            .env(
                "AMPLIHACK_AUTODRIVE_READY",
                String::from_utf8(ready.stdout).unwrap(),
            )
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let output: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(output["repo"], f.repo.to_str().unwrap());
        assert!(!f.tmp.path().join("comments").exists());
    }
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
        let ready = f.run();
        assert!(ready.status.success());
        let o = f
            .command("bash")
            .current_dir(f.tmp.path())
            .args(["-c", yaml["steps"][0]["command"].as_str().unwrap()])
            .env("PR_NUMBER", "7")
            .env("REPO_PATH", "repo")
            .env(
                "AMPLIHACK_AUTODRIVE_READY",
                String::from_utf8(ready.stdout).unwrap(),
            )
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

#[test]
fn runner_default_state_key_removes_only_one_git_suffix() {
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let f = Fixture::new();
        f.install_loop_tools();
        f.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/source/project.git.git",
        ]);
        fs::write(f.repo.join("dirty"), "preserve").unwrap();
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
                "--format",
                "json",
            ])
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .output()
            .unwrap();
        assert!(!o.status.success());
        let state = f.tmp.path().join("AMPLIHACK_STATE_DIR/auto-drive/https___github.com_source_project.git_7/preflight-result.json");
        let envelope: Value = serde_json::from_slice(&fs::read(&state).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; {}",
                state.display(),
                String::from_utf8_lossy(&o.stderr)
            )
        }))
        .unwrap();
        assert_eq!(envelope["outcome"], "pr-worktree-dirty");
    }
}
