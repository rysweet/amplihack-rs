//! External boundary executable for the #2553 red-phase integration tests.
//! It supplies agent/service responses, not orchestration or retry logic.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[path = "issue_2553_fixture/phase.rs"]
mod phase;

fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("ISSUE_2553_ROOT").expect("isolated fixture root"))
}

fn owned_file(path: impl AsRef<std::path::Path>) -> Result<PathBuf> {
    let path = path.as_ref().canonicalize()?;
    anyhow::ensure!(
        path.starts_with(root()),
        "fixture file escaped its owned root"
    );
    Ok(path)
}

fn record(event: Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root().join("events.jsonl"))?;
    let mut bytes = serde_json::to_vec(&event)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn result(context: &Value) -> Value {
    let key = context["workstream_id"]
        .as_str()
        .unwrap_or("missing-identity");
    let override_file = root().join(format!("result-{key}.json"));
    if override_file.exists() {
        return serde_json::from_slice(&fs::read(override_file).unwrap()).unwrap();
    }
    json!({"schema_version": 1, "workstream_id": key,
        "repository": context.get("repository").unwrap_or(&Value::Null),
        "classification": "Operations", "effective_recipe": "operations-direct",
        "outcome": "succeeded", "outputs": {"answer": {"kind": "answer",
            "text": "Fixture research answer", "destination": "chat",
            "evidence": [{"source": "fixture", "reference": "research-answer",
                "observation": "Answered the requested research question"}]}}})
}

fn snapshot_states() -> Result<Vec<Value>> {
    let state_dir = root().join("tmp/amplihack-workstreams/state");
    if !state_dir.exists() {
        return Ok(vec![]);
    }
    let mut states = vec![];
    for entry in fs::read_dir(state_dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "json") {
            states.push(serde_json::from_slice(&fs::read(owned_file(path)?)?)?);
        }
    }
    Ok(states)
}

fn execute_external(args: &[String]) -> Result<()> {
    let context_file = args
        .iter()
        .position(|a| a == "--context-file")
        .and_then(|i| args.get(i + 1))
        .context("launcher must pass --context-file")?;
    let context: Value = serde_json::from_slice(&fs::read(owned_file(context_file)?)?)?;
    let recipe = args.get(2).context("recipe name")?;
    let event = json!({"kind": "execute", "recipe": recipe, "context": context,
        "cwd": std::env::current_dir()?, "context_file": context_file,
        "states_at_launch": snapshot_states()?,
        "inherited_issue": std::env::var("RECIPE_VAR_issue_number").ok(),
        "inherited_url": std::env::var("RECIPE_VAR_issue_url").ok(),
        "workstream_issue": std::env::var("AMPLIHACK_WORKSTREAM_ISSUE").ok()});
    record(event)?;
    if recipe != "operations-direct" {
        record(json!({"kind": "development", "recipe": recipe}))?;
        bail!("unexpected Development workflow for Operations fixture");
    }
    // Tests can force a definite child failure independently of result validity.
    if root().join("agent-failure").exists() {
        bail!("fixture agent rejected task");
    }
    if fs::read_to_string(root().join("break-result-persistence"))
        .ok()
        .is_some_and(|id| Some(id.as_str()) == context["workstream_id"].as_str())
    {
        let state = PathBuf::from(
            context["workstream_state_file"]
                .as_str()
                .context("state path")?,
        );
        owned_file(&state)?;
        if state.is_file() {
            fs::remove_file(&state)?;
        }
        fs::create_dir(&state)?;
    }
    let request = root().join("operation-request.json");
    let result = if request.exists() {
        let output_name = "created_issue";
        let private_request = std::path::Path::new(context_file)
            .parent()
            .context("context parent")?
            .join("operation-request.json");
        fs::copy(&request, &private_request)?;
        amplihack_cli::commands::multitask::run_operation(
            context_file,
            output_name,
            private_request.to_str().context("request path")?,
        )?;
        let saved: Value = serde_json::from_slice(&fs::read(owned_file(
            context["workstream_state_file"]
                .as_str()
                .context("state file")?,
        )?)?)?;
        let output = saved["operation_attempts"]
            .as_array()
            .context("saved operations")?
            .last()
            .context("saved operation")?["output"]
            .clone();
        json!({"schema_version": 1, "workstream_id": context["workstream_id"], "repository": context["repository"],
            "classification": "Operations", "effective_recipe": "operations-direct", "outcome": "succeeded", "outputs": {output_name: output}})
    } else {
        result(&context)
    };
    for field in ["workstream_result_file", "result_file"] {
        if let Some(path) = context[field].as_str() {
            let path = PathBuf::from(path);
            let parent = path
                .parent()
                .context("result parent directory")?
                .canonicalize()?;
            anyhow::ensure!(
                parent.starts_with(root()),
                "result path escaped fixture root"
            );
            anyhow::ensure!(
                !path.is_symlink(),
                "fixture result file must not be a symlink"
            );
            fs::write(path, serde_json::to_vec(&result)?)?;
        }
    }
    println!("{result}");
    eprint!("fixture stderr tail without newline");
    Ok(())
}

fn execute_classic(args: &[String]) -> Result<()> {
    let mut input = args.join(" ");
    for token in args.iter().flat_map(|arg| arg.split_whitespace()) {
        if let Some(path) = token.strip_prefix('@') {
            input.push_str(&fs::read_to_string(owned_file(path)?)?);
        }
    }
    let state_file = std::env::var("AMPLIHACK_WORKSTREAM_STATE_FILE")
        .context("classic launcher must identify its owned state")?;
    let state: Value = serde_json::from_slice(&fs::read(owned_file(state_file)?)?)?;
    let context =
        json!({"workstream_id": state["workstream_id"], "repository": state["repository"]});
    record(
        json!({"kind": "execute", "recipe": state["recipe"], "raw_input": input,
        "cwd": std::env::current_dir()?, "context": context}),
    )?;
    println!("{}", result(&context));
    Ok(())
}

fn git(args: &[String]) -> Result<()> {
    if args.iter().any(|a| {
        [
            "clone",
            "worktree",
            "checkout",
            "switch",
            "branch",
            "push",
            "fetch",
            "pull",
            "ls-remote",
            "commit",
            "reset",
            "clean",
            "merge",
            "rebase",
            "tag",
        ]
        .contains(&a.as_str())
    }) {
        record(json!({"kind": "forbidden_git", "args": args}))?;
        bail!("Operations must not create a clone, worktree or branch");
    }
    let out = Command::new("/usr/bin/git")
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    std::io::stdout().write_all(&out.stdout)?;
    std::io::stderr().write_all(&out.stderr)?;
    anyhow::ensure!(out.status.success(), "git read failed");
    Ok(())
}

fn gh(args: &[String]) -> Result<()> {
    let mut body = String::new();
    if let Some(i) = args.iter().position(|a| a == "--body-file") {
        let path = args.get(i + 1).context("body file argument")?;
        if path == "-" {
            std::io::stdin().read_to_string(&mut body)?;
        } else {
            body = fs::read_to_string(owned_file(path)?)?;
        }
    }
    let states = snapshot_states()?;
    record(
        json!({"kind": if args.first().is_some_and(|a| a == "pr") { "pr" } else { "gh" },
        "args": args, "body": body, "states_at_invocation": states,
        "host": std::env::var("GH_HOST").ok(),
        "repo_override": std::env::var("GH_REPO").ok(),
        "debug": std::env::var("GH_DEBUG").ok(),
        "prompt_disabled": std::env::var("GH_PROMPT_DISABLED").ok()}),
    )?;
    let mode = fs::read_to_string(root().join("gh-mode")).unwrap_or_else(|_| "success".into());
    match mode.trim() {
        "rejected" => bail!("fixture: permission denied; no issue created"),
        "uncertain" => bail!("fixture: response lost after server accepted issue creation"),
        "malformed" => println!("not-an-issue"),
        "wrong-repository" => println!("https://github.com/example/foreign/issues/2553"),
        "internal-whitespace" => println!("https://github.com/example/operations/issues/ 2553"),
        "oversized" => println!("{}", "x".repeat(128 * 1024)),
        "invalid-utf8" => std::io::stdout().write_all(&[0xff])?,
        "sensitive-diagnostic" => bail!("fixture-secret-token"),
        _ => println!("https://github.com/example/operations/issues/2553"),
    }
    Ok(())
}

fn main_result() -> Result<()> {
    let mut args = std::env::args();
    let program = PathBuf::from(args.next().context("argv0")?);
    let args: Vec<String> = args.collect();
    match program.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        "git" => return git(&args),
        "gh" => return gh(&args),
        "amplihack" if args.starts_with(&["recipe".into(), "run".into()]) => {
            return execute_external(&args);
        }
        "amplihack" if args.first().is_some_and(|a| a == "claude") => {
            return execute_classic(&args);
        }
        "claude" => return execute_classic(&args),
        "amplihack" => {
            let cli = amplihack_cli::Cli::try_parse_from(
                std::iter::once("amplihack".to_owned()).chain(args.iter().cloned()),
            )?;
            return amplihack_cli::commands::dispatch(cli.command);
        }
        _ => {}
    }
    match args.first().map(String::as_str) {
        Some("normalise") => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let path = amplihack_cli::commands::orch::build_workstreams_config_with_override(
                &input,
                args.get(1).map(String::as_str).unwrap_or(""),
            )?;
            let body = fs::read(&path)?;
            fs::remove_file(path)?;
            std::io::stdout().write_all(&body)?;
        }
        Some("execute") => amplihack_cli::commands::multitask::run_multitask(
            args.get(1).context("config path")?,
            args.get(2).map(String::as_str).unwrap_or("recipe"),
            "default-workflow",
            None,
            None,
            false,
        )?,
        Some("cli") => {
            let cli = amplihack_cli::Cli::try_parse_from(
                std::iter::once("amplihack".to_owned()).chain(args.iter().skip(1).cloned()),
            )?;
            amplihack_cli::commands::dispatch(cli.command)?;
        }
        Some("phase") => phase::run(
            args.get(1).context("phase context file")?,
            "smart-execute-routing",
        )?,
        Some("operations-recipe") => phase::run(
            args.get(1).context("Operations context file")?,
            "operations-direct",
        )?,
        _ => bail!("unsupported fixture invocation: {args:?}"),
    }
    Ok(())
}

fn main() {
    if let Err(error) = main_result() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
