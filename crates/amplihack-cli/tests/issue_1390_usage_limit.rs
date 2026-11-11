//! Exercise the real CLI with a compiled Rust runner, without an account,
//! quota probes, polling, or access to the developer's home.
//! A `.git` boundary at the fixture root stops the launcher-context walk there.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const MESSAGE: &str = "Agent terminated early due to an API error: You've hit your session limit · resets 9:20am (UTC)";

fn isolated_command(binary: impl AsRef<std::ffi::OsStr>, root: &Path) -> Command {
    let home = root.join("home");
    let mut command = crate::isolated_amplihack::isolated(binary);
    command
        .current_dir(root.join("repo"))
        // Keep HOME and the fake proc tree inside the child's guarded temp
        // boundary even when the parent recipe supplies a different TMPDIR.
        .env("TMPDIR", root)
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", home.join(".amplihack"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env(
            crate::isolated_amplihack::PROFILE_ROOT_ENV,
            home.join("profiles"),
        )
        .env(crate::isolated_amplihack::PROC_ROOT_ENV, root.join("proc"))
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1");
    command
}

// Use the pinned runner's recovery types and private storage implementation.
// Probes have no side effects. Setup never launches another agent.
const RUNNER_SOURCE: &str = r##"
use recipe_runner_rs::recovery::{model::Descriptor, private_fs::{Directory, parse}, store::Store};
use std::{env, fs, io::{self, Write}, path::PathBuf, process};

fn required(name: &str) -> PathBuf {
    PathBuf::from(env::var_os(name).unwrap_or_else(|| panic!("missing {name}")))
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args == ["--version"] {
        println!("recipe-runner-rs 0.0.0 (fixture)");
        return Ok(());
    }
    if args == ["--capabilities"] {
        println!(r#"{{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}}"#);
        return Ok(());
    }
    let recipe = required("ISSUE_1390_RECIPE");
    let work = required("ISSUE_1390_WORK");
    let received_recipe = PathBuf::from(args.first().expect("recipe argument"));
    assert_eq!(received_recipe.canonicalize()?, recipe.canonicalize()?);
    assert!(args.windows(2).any(|p| p == ["--output-format", "json"]));
    let received_work = args.windows(2).find(|p| p[0] == "-C")
        .expect("working-directory argument");
    assert_eq!(PathBuf::from(&received_work[1]).canonicalize()?, work.canonicalize()?);
    assert!(work.join(".git").is_dir());
    assert_eq!(env::var("AMPLIHACK_AGENT_BINARY").expect("selected agent"), "claude");

    // Admission must precede the invocation ledger, setup and result output.
    // Read through the same private directory API as the real runner; opening
    // the Store validates the descriptor and acquires its exclusive writer.
    let descriptor_path = PathBuf::from(&args.windows(2)
        .find(|p| p[0] == "--recovery-descriptor")
        .expect("missing --recovery-descriptor")[1]);
    let parent = Directory::open(descriptor_path.parent().expect("descriptor parent"), false)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let descriptor: Descriptor = parse(&parent.read(descriptor_path.file_name()
        .and_then(|n| n.to_str()).expect("descriptor name"))
        .map_err(|e| io::Error::other(e.to_string()))?)
        .map_err(|e| io::Error::other(e.to_string()))?;
    assert_eq!(descriptor.directory.canonicalize()?, descriptor_path.parent().unwrap().canonicalize()?);
    let store = Store::open(descriptor).map_err(|e| io::Error::other(e.to_string()))?;
    let payload = fs::read(required("ISSUE_1390_PAYLOAD"))?;
    let mut record = fs::OpenOptions::new().create(true).append(true)
        .open(required("ISSUE_1390_RECORD"))?;
    writeln!(record, "run")?;
    if let Some(setup) = env::var_os("ISSUE_1390_SETUP") {
        fs::write(setup, "setup complete\n")?;
    }
    io::stdout().write_all(&payload)?;
    io::stdout().flush()?;
    drop(store);
    process::exit(1);
}
"##;

fn runner_library(root: &Path) -> PathBuf {
    static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            // Cargo's artifact report identifies the current pinned dependency;
            // guessing an rlib from deps/ could pick a stale runner revision.
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let output = isolated_command(cargo, root)
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .args([
                    "build",
                    "--locked",
                    "--offline",
                    "-p",
                    "recipe-runner-rs",
                    "--lib",
                    "--message-format=json",
                ])
                .output()
                .expect("resolve the pinned runner library");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("Cargo artifact JSON"))
                .filter(|artifact| {
                    artifact["reason"] == "compiler-artifact"
                        && artifact["target"]["name"] == "recipe_runner_rs"
                })
                .flat_map(|artifact| artifact["filenames"].as_array().unwrap().clone())
                .filter_map(|name| name.as_str().map(PathBuf::from))
                .find(|path| path.extension().is_some_and(|ext| ext == "rlib"))
                .expect("Cargo did not report the pinned runner rlib")
        })
        .clone()
}

fn compile_runner(root: &Path) -> PathBuf {
    let source = root.join("runner.rs");
    fs::write(&source, RUNNER_SOURCE).unwrap();
    let runner = root.join("recipe-runner-rs");
    let library = runner_library(root);
    let output = isolated_command("rustc", root)
        .arg("--edition=2024")
        .arg("-L")
        .arg(format!(
            "dependency={}",
            library.parent().unwrap().join("deps").display()
        ))
        .arg("--extern")
        .arg(format!("recipe_runner_rs={}", library.display()))
        .arg(&source)
        .arg("-o")
        .arg(&runner)
        .output()
        .expect("compile Rust recipe fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    runner
}

fn assert_agent_resolution(binary: &Path, root: &Path, expected: &str) {
    let output = isolated_command(binary, root)
        .args(["agent-binary", "-w"])
        .arg(root.join("repo"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    crate::isolated_amplihack::assert_proc_root_honoured(&stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        expected,
        "{stderr}"
    );
}

fn fixture_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
    let root = tmp.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    // Stop every ancestor lookup inside owned storage, including probes made
    // before the inner repository boundary exists.
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join("home")).unwrap();
    fs::create_dir(root.join("repo")).unwrap();
    fs::create_dir(root.join("proc")).unwrap();
    crate::isolated_amplihack::seed_proc_tree(&root.join("proc"));
    tmp
}

#[test]
fn runner_probes_advertise_durable_recovery_without_performing_work() {
    let tmp = fixture_root();
    let root = tmp.path();
    let runner = compile_runner(root);
    let record = root.join("invocations");
    let setup = root.join("setup-complete");
    for probe in ["--version", "--capabilities"] {
        let output = isolated_command(&runner, root)
            .arg(probe)
            .env("AMPLIHACK_WORK_ROOT", root.join("work"))
            .env("ISSUE_1390_RECORD", &record)
            .env("ISSUE_1390_SETUP", &setup)
            .output()
            .unwrap();
        assert!(output.status.success(), "{probe}: {output:?}");
        assert!(output.stderr.is_empty(), "{probe}: {output:?}");
        assert!(!record.exists(), "{probe} counted as a recipe execution");
        assert!(!setup.exists(), "{probe} performed recipe setup");
        assert!(
            !root.join("work").exists(),
            "{probe} created recovery storage"
        );
        if probe == "--version" {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                "recipe-runner-rs 0.0.0 (fixture)"
            );
        } else {
            let capabilities: Value = serde_json::from_slice(&output.stdout)
                .expect("fixture capability probe must emit valid JSON");
            assert_eq!(capabilities["schema_version"], 1);
            assert!(
                capabilities["version"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty())
            );
            let advertised = capabilities["capabilities"]
                .as_array()
                .expect("capabilities array");
            for required in ["codex_exec", "durable_recovery_v1"] {
                assert!(advertised.iter().any(|v| v == required), "{capabilities}");
            }
        }
    }
}

#[test]
fn runner_rejects_missing_or_invalid_recovery_descriptors_before_work() {
    let tmp = fixture_root();
    let root = tmp.path();
    let repo = root.join("repo");
    fs::create_dir(repo.join(".git")).unwrap();
    let recipe = repo.join("recipe.yaml");
    fs::write(&recipe, "name: issue-1390\nsteps: []\n").unwrap();
    let payload = root.join("payload.json");
    fs::write(&payload, "{\"success\":false}\n").unwrap();
    let record = root.join("invocations");
    let setup = root.join("setup-complete");
    let runner = compile_runner(root);
    let invalid_json = root.join("invalid-json.json");
    let invalid_descriptor = root.join("invalid-descriptor.json");
    fs::write(&invalid_json, "not JSON").unwrap();
    fs::write(&invalid_descriptor, "{}").unwrap();
    for path in [&invalid_json, &invalid_descriptor] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // No alternative recovery schema here: malformed JSON, an absent file,
    // and an object that cannot decode as the existing Descriptor must fail.
    for (case, descriptor, diagnostic) in [
        ("missing argument", None, None),
        ("missing file", Some(root.join("absent.json")), None),
        (
            "invalid JSON",
            Some(invalid_json.canonicalize().unwrap()),
            Some("expected ident"),
        ),
        (
            "invalid descriptor",
            Some(invalid_descriptor.canonicalize().unwrap()),
            Some("missing field"),
        ),
    ] {
        let mut command = isolated_command(&runner, root);
        command
            .arg(&recipe)
            .args(["--output-format", "json", "-C"])
            .arg(&repo)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("ISSUE_1390_RECIPE", &recipe)
            .env("ISSUE_1390_WORK", &repo)
            .env("ISSUE_1390_PAYLOAD", &payload)
            .env("ISSUE_1390_RECORD", &record)
            .env("ISSUE_1390_SETUP", &setup);
        if let Some(descriptor) = descriptor {
            command.arg("--recovery-descriptor").arg(descriptor);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{case}: {output:?}");
        assert!(!record.exists(), "{case}: execution preceded admission");
        assert!(!setup.exists(), "{case}: setup preceded admission");
        assert!(output.stdout.is_empty(), "{case}: emitted a recipe result");
        assert!(!output.stderr.is_empty(), "{case}: no failure diagnostic");
        if let Some(expected) = diagnostic {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expected), "{case}: {stderr}");
        }
    }
}

#[test]
fn a_single_limited_step_preserves_the_reset_hint() {
    run_reported_limit(false);
}

#[test]
fn reported_limit_survives_the_cli_boundary_without_transport_retry() {
    run_reported_limit(true);
}

fn run_reported_limit(include_setup: bool) {
    // Under a recipe run TMPDIR lies inside ~/.amplihack, whose `.claude/`
    // and `amplifier-bundle/` the CLI's ancestor scans would find ahead of
    // this fixture's (tests/support/scratch_base.rs).
    let tmp = fixture_root();
    let root = tmp.path();
    // The outer boundary contains both probes, even before the inner boundary
    // exists. No launcher lookup can reach state outside this private tree.
    let repo = root.join("repo");
    let runtime = root.join(".claude/runtime");
    fs::create_dir_all(&runtime).unwrap();
    // Future timestamp avoids a freshness window: no elapsed-time assertions.
    fs::write(
        runtime.join("launcher_context.json"),
        r#"{"launcher":"codex","timestamp":"9999-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let binary = amplihack_bin();
    assert_agent_resolution(&binary, root, "codex (launcher_context)");
    fs::create_dir(repo.join(".git")).unwrap();
    assert_agent_resolution(&binary, root, "copilot (default)");

    let runner = compile_runner(root);
    let payload = root.join("payload.json");
    let record = root.join("invocations");
    let setup = root.join("setup-complete");
    let mut steps = Vec::new();
    if include_setup {
        steps.push(serde_json::json!({"step_id": "setup", "status": "completed"}));
    }
    steps.push(serde_json::json!({
        "step_id": "limited-agent",
        "status": "error",
        "error": "agent step failed (exit 1)",
        "recent_stdout": [MESSAGE]
    }));
    let result = serde_json::json!({
        "recipe_name": "issue-1390",
        "success": false,
        "step_results": steps,
        "context": {}
    });
    fs::write(&payload, format!("{result}\n")).unwrap();
    let recipe = repo.join("recipe.yaml");
    let setup_step = if include_setup {
        "  - id: setup\n    type: bash\n    command: 'fixture filesystem setup'\n"
    } else {
        ""
    };
    fs::write(&recipe, format!("name: issue-1390\ndescription: Synthetic usage limit\nversion: 1.0.0\nsteps:\n{setup_step}  - id: limited-agent\n    type: agent\n    prompt: 'Synthetic usage limit'\n")).unwrap();
    let mut command = isolated_command(&binary, root);
    command
        .args(["recipe", "run"])
        .arg(&recipe)
        .args(["--format", "json", "-w"])
        .arg(&repo)
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        .env("AMPLIHACK_WORK_ROOT", root.join("work"))
        // Boundary isolation was proved without this override above. Keep
        // agent selection deterministic for this separate recipe scenario.
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("ISSUE_1390_RECORD", &record)
        .env("ISSUE_1390_PAYLOAD", &payload)
        .env("ISSUE_1390_RECIPE", &recipe)
        .env("ISSUE_1390_WORK", &repo);
    if include_setup {
        command.env("ISSUE_1390_SETUP", &setup);
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    crate::isolated_amplihack::assert_proc_root_honoured(&stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let pointers: Vec<Value> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("amplihack.recipe.log_pointer "))
        .map(|payload| serde_json::from_str(payload).expect("log pointer JSON"))
        .collect();
    assert!(!pointers.is_empty(), "{stderr}");
    for pointer in &pointers {
        assert_eq!(pointer["agent_binary"], "claude", "{pointer}");
        assert_eq!(pointer["agent_binary_source"], "env", "{pointer}");
    }
    let structured: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={} stderr={stderr}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(structured["success"], false);
    let verdict = &structured["failure_classification"];
    assert_eq!(verdict["class"], "usage_limit");
    assert_eq!(verdict["signal"], "session limit");
    assert_eq!(verdict["retryable"], false);
    assert_eq!(verdict["attempt"], 1);
    assert_eq!(verdict["action"], "terminal");
    let completed: Vec<&str> = if include_setup { vec!["setup"] } else { vec![] };
    assert_eq!(verdict["completed_steps"], serde_json::json!(completed));
    assert_eq!(
        verdict["failed_steps"],
        serde_json::json!(["limited-agent"])
    );
    assert!(
        verdict["reasoning"]
            .as_str()
            .unwrap()
            .contains("9:20am (UTC)")
    );
    assert!(verdict["evidence"].as_str().unwrap().contains(MESSAGE));
    assert!(
        verdict["evidence"]
            .as_str()
            .unwrap()
            .contains("agent step failed (exit 1)")
    );
    let markers: Vec<Value> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("amplihack.recipe.failure_class "))
        .map(|payload| serde_json::from_str(payload).expect("structured diagnostic marker"))
        .collect();
    assert_eq!(markers, vec![verdict.clone()], "{stderr}");
    // Process completion is the synchronization event; no timed window.
    assert_eq!(fs::read_to_string(record).unwrap(), "run\n");
    if include_setup {
        assert_eq!(fs::read_to_string(setup).unwrap(), "setup complete\n");
    } else {
        assert!(!setup.exists());
    }

    // A capability string alone cannot establish successful admission. Both
    // scenarios must leave the current main runner contract's durable launch
    // artifacts inside the fixture, finalized by the trusted CLI classifier.
    let runs: Vec<_> = fs::read_dir(root.join("work/runs"))
        .expect("private durable run storage")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1, "one durably admitted launch: {runs:?}");
    let recovery = runs[0].join("recovery");
    let descriptor: Value = serde_json::from_slice(
        &fs::read(recovery.join("descriptor.json")).expect("durable launch descriptor"),
    )
    .unwrap();
    let tree: Value = serde_json::from_slice(
        &fs::read(recovery.join("tree.json")).expect("durable launch outcome"),
    )
    .unwrap();
    assert_eq!(descriptor["schema_version"], 1);
    assert_eq!(descriptor["sequence"], 1);
    assert_eq!(
        descriptor["run_id"],
        runs[0].file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(descriptor["fresh"], false);
    assert!(descriptor["source"].is_null());
    assert_eq!(
        Path::new(descriptor["directory"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        recovery.canonicalize().unwrap()
    );
    assert_eq!(tree["run_id"], descriptor["run_id"]);
    assert_eq!(tree["schema_version"], 1);
    assert_eq!(tree["sequence"], descriptor["sequence"]);
    assert_eq!(tree["outcome"], "usage_limit");
    assert!(
        tree["revision"].as_u64().unwrap() >= 2,
        "runner writer and CLI finalization: {tree}"
    );
}
