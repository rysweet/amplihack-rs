//! External Rust example fixture, never an alternative orchestrator implementation.
//!
//! `driver` calls the public production entry point. Symlinks named `git`,
//! `gh` and `amplihack` record external actions; unexpected actions fail closed.
//! Completion uses process exits and files written before those exits.

mod issue_2553_tracking_adapter;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn root() -> PathBuf {
    PathBuf::from(env::var_os("ISSUE_2553_ROOT").expect("private fixture root"))
}

fn record(value: Value) -> Result<()> {
    let bytes = format!("{value}\n");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root().join("actions.jsonl"))?
        .write_all(bytes.as_bytes())?;
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let program = env::args().next().unwrap();
    let program = Path::new(&program).file_name().unwrap().to_string_lossy();
    // Fail before any service action if the harness ever loses isolation.
    for name in [
        "HOME",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    ] {
        let value = PathBuf::from(env::var_os(name).with_context(|| format!("missing {name}"))?);
        ensure!(
            value.starts_with(root()),
            "{name} escaped the private fixture"
        );
    }
    match program.as_ref() {
        "git" => {
            record(json!({"action": "git", "args": args}))?;
            let command = args.iter().find(|arg| {
                matches!(
                    arg.as_str(),
                    "clone" | "ls-remote" | "remote" | "symbolic-ref" | "rev-parse"
                )
            });
            match command.map(String::as_str) {
                Some("remote") => println!(
                    "{}",
                    env::var("ISSUE_2553_ORIGIN")
                        .unwrap_or_else(|_| root().join("repository").display().to_string())
                ),
                Some("ls-remote") => println!("ref: refs/heads/main\tHEAD"),
                Some("symbolic-ref") => println!("origin/main"),
                Some("rev-parse") => {
                    let cwd = env::current_dir()?;
                    if args.iter().any(|arg| arg == "--git-common-dir") {
                        println!(
                            "{}\n{}\n{}",
                            env::var_os("ISSUE_2553_COMMON_DIR")
                                .map(PathBuf::from)
                                .unwrap_or_else(|| cwd.join(".git"))
                                .display(),
                            cwd.join(".git").display(),
                            cwd.display()
                        );
                    } else {
                        println!("{}", cwd.display());
                    }
                }
                Some("clone") => {
                    let destination = PathBuf::from(args.last().context("clone destination")?);
                    ensure!(destination.starts_with(root()), "clone escaped the fixture");
                    ensure!(
                        env::var("ISSUE_2553_DEVELOPMENT_KEY")
                            .ok()
                            .is_some_and(|key| destination
                                .file_name()
                                .is_some_and(|name| name == format!("ws-{key}").as_str())),
                        "fixture forbids development cloning for this Operations execution"
                    );
                    fs::create_dir_all(destination.join(".git"))?;
                }
                _ => bail!("unexpected fixture git command: {args:?}"),
            }
        }
        "gh" => {
            record(json!({"action": "gh", "args": args}))?;
            if args.first().is_some_and(|arg| arg == "pr") {
                eprintln!("fixture-secret-token: cleanup authentication failure");
                std::process::exit(17);
            }
            issue_2553_tracking_adapter::run(&args)?;
        }
        "amplihack" if args.first().is_some_and(|arg| arg == "recipe") => {
            ensure!(
                args.get(1).map(String::as_str) == Some("run"),
                "unexpected recipe command"
            );
            let recipe = args.get(2).context("recipe name")?;
            let context_path = args
                .windows(2)
                .find(|pair| pair[0] == "--context-file")
                .map(|pair| &pair[1])
                .context("context-file argument")?;
            let context: Value = serde_json::from_slice(&fs::read(context_path)?)?;
            let persisted_state = context
                .get("workstream_state_file")
                .and_then(Value::as_str)
                .map(fs::read)
                .transpose()?
                .map(|bytes| serde_json::from_slice::<Value>(&bytes))
                .transpose()?;
            record(
                json!({"action": "execute", "recipe": recipe, "context": context,
                          "cwd": env::current_dir()?, "delegate": env::var_os("AMPLIHACK_DELEGATE").map(|value| value.to_string_lossy().into_owned()),
                          "persisted_state": persisted_state,
                          "inherited_recipe": env::var_os("RECIPE_VAR_recipe"),
                          "inherited_issue": env::var_os("AMPLIHACK_WORKSTREAM_ISSUE"),
                          "inherited_create_issue": env::var_os("AMPLIHACK_CREATE_ISSUE")}),
            )?;
            if let Some(socket) = env::var_os("ISSUE_2553_READY_SOCKET") {
                use std::io::Read;
                // The parent closes stdin after committing its running state.
                // Waiting for EOF gives this fixture an event-driven barrier.
                std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
                let mut connection = std::os::unix::net::UnixStream::connect(socket)?;
                let state: Value = serde_json::from_slice(&fs::read(
                    context["workstream_state_file"].as_str().unwrap(),
                )?)?;
                ensure!(
                    state["lifecycle_state"] == "running",
                    "running checkpoint was not committed"
                );
                connection.write_all(b"ready")?;
                let mut release = [0];
                connection.read_exact(&mut release)?;
            }
            let status = env::var("ISSUE_2553_AGENT_STATUS").unwrap_or_else(|_| "completed".into());
            let result = format!("OPERATIONS_RESULT: {status} — fixture answer");
            if let Some(path) = context
                .get("workstream_progress_file")
                .and_then(Value::as_str)
                .filter(|_| env::var_os("ISSUE_2553_NO_SIDECAR").is_none())
            {
                fs::write(
                    path,
                    serde_json::to_vec(&json!({"lifecycle_state": status,
                    "current_step": "execute-operation", "operations_result": result}))?,
                )?;
            }
            record(
                json!({"action": "completed", "task": context["task_description"], "result": result}),
            )?;
            if args.windows(2).any(|pair| pair == ["--format", "json"]) {
                println!(
                    "{}",
                    json!({"success": env::var_os("ISSUE_2553_RUNNER_FAILURE").is_none(),
                    "step_results": [{"step_id": "execute-operation", "output": result}],
                    "recipe": "untrusted-override", "classification": "Development",
                    "source_repository": {"worktree": "/foreign"}})
                );
            } else {
                println!("{result}");
            }
            ensure!(
                env::var("ISSUE_2553_AGENT_EXIT").ok().as_deref() != Some("17"),
                "fixture process exit overrides successful prose"
            );
        }
        _ if args.first().is_some_and(|arg| arg == "driver") => {
            let config = args.get(1).context("driver config")?;
            amplihack_cli::commands::multitask::run_multitask(
                config,
                "recipe",
                "default-workflow",
                None,
                None,
                false,
            )?;
        }
        _ if args.first().is_some_and(|arg| arg == "cleanup") => {
            amplihack_cli::commands::multitask::run_cleanup(
                args.get(1).context("cleanup config")?,
                false,
            )?;
        }
        _ => bail!("unexpected fixture invocation: {program} {args:?}"),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(17);
    }
}
