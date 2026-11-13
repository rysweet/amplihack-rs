//! Durable issue mutation adapter for the single Operations agent.
use super::{
    completion, github,
    identity::WorkstreamId,
    issue::parse_optional,
    runtime::{self, Runtime},
};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::path::Path;

pub fn run(context_file: &Path, output_name: &str, request_file: &Path) -> Result<()> {
    runtime::owned(context_file, &runtime::base_dir())?;
    let context: Value = serde_json::from_slice(&fs::read(context_file)?)?;
    let state_path = Path::new(
        context["workstream_state_file"]
            .as_str()
            .context("missing state file")?,
    );
    runtime::owned(state_path, &runtime::base_dir().join("state"))?;
    let mut state: Runtime = serde_json::from_slice(&fs::read(state_path)?)?;
    ensure!(
        state.config.classification == "Operations"
            && context["workstream_id"] == json!(state.id())
            && state.lifecycle == "running",
        "operation does not belong to a running Operations leaf"
    );
    let lock_path = state.artefact_dir.join("mutation.lock");
    runtime::owned(&lock_path, &state.artefact_dir)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lock_path)?;
    lock.try_lock_exclusive()
        .context("another mutation owns this leaf")?;
    // Reload under the leaf lock, as another completed adapter may have advanced it.
    state = serde_json::from_slice(&fs::read(state_path)?)?;
    ensure!(
        state.config.classification == "Operations"
            && context["workstream_id"] == json!(state.id())
            && context["repository"] == state.config.repository
            && state.state_file == state_path
            && state.lifecycle == "running",
        "mutation context ownership or lifecycle mismatch"
    );
    runtime::owned(request_file, &state.artefact_dir)?;
    let request: Value = serde_json::from_slice(&fs::read(request_file)?)?;
    let kind = state
        .config
        .required_outputs
        .get(output_name)
        .context("undeclared operation output")?
        .clone();
    for previous in state
        .operation_attempts
        .iter()
        .filter(|a| a["output_name"] == output_name)
    {
        ensure!(
            previous["request"] == request,
            "operation request changed for output {output_name}"
        );
        if previous["status"] == "succeeded" {
            println!("{}", previous["output"]);
            return Ok(());
        }
        ensure!(
            previous["status"] == "not_applied",
            "operation requires reconciliation before another attempt"
        );
    }
    let action = request["action"]
        .as_str()
        .context("operation action is required")?;
    let args = &request["arguments"];
    let mut command = github::command(&state.work_dir);
    let mut body = None;
    let target = match action {
        "create_issue" => {
            ensure!(kind == "issue", "creation requires an issue output");
            completion::non_blank(args, "title")?;
            completion::non_blank(args, "body")?;
            command.args([
                "issue",
                "create",
                "--repo",
                &state.config.repository,
                "--title",
                args["title"].as_str().unwrap(),
                "--body-file",
                "-",
            ]);
            body = Some(args["body"].as_str().unwrap().to_owned());
            None
        }
        "assign" => {
            ensure!(
                kind == "operation",
                "assignment requires an operation output"
            );
            let number = parse_optional(&args["issue"])?.context("assignment issue is required")?;
            let assignees = args["assignees"]
                .as_array()
                .filter(|a| !a.is_empty())
                .context("assignment needs assignees")?;
            command.args([
                "issue",
                "edit",
                &number.get().to_string(),
                "--repo",
                &state.config.repository,
            ]);
            for assignee in assignees {
                let login = assignee
                    .as_str()
                    .filter(|s| {
                        !s.is_empty()
                            && !s.starts_with('-')
                            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
                    .context("invalid assignee login")?;
                command.args(["--add-assignee", login]);
            }
            Some(number)
        }
        _ => bail!("unsupported issue operation {action:?}"),
    };
    state.operation_attempts.push(json!({"schema_version": 1, "attempt_id": WorkstreamId::new(), "workstream_id": state.id(), "repository": state.config.repository,
        "revision": state.revision + 1, "purpose": "operation", "request": request, "output_name": output_name, "status": "prepared"}));
    state.save()?;
    let executed = (|| -> Result<Value> {
        let response = github::execute(&mut command, body.as_deref(), &state.artefact_dir)?;
        let number = if let Some(number) = target {
            number
        } else {
            github::created_issue(&response, &state.config.repository)?.number
        };
        let issue = json!({"repository": state.config.repository, "number": number});
        let evidence = json!([{"source": "github", "reference": format!("https://github.com/{}/issues/{}", state.config.repository, number.get()), "observation": format!("Accepted {action} for declared output {output_name}")}]);
        let output = if action == "create_issue" {
            json!({"kind": "issue", "issue": issue, "evidence": evidence})
        } else {
            json!({"kind": "operation", "action": action, "issue": issue, "evidence": evidence})
        };
        completion::output(&output, &kind, &state.config.repository)?;
        Ok(output)
    })();
    let attempt = state.operation_attempts.last_mut().unwrap();
    match executed {
        Ok(output) => {
            attempt["status"] = json!("succeeded");
            attempt["output"] = output.clone();
            state.save()?;
            println!("{output}");
            Ok(())
        }
        Err(error) => {
            attempt["status"] = json!("reconciliation_required");
            attempt["diagnostic"] = json!(format!("{error:#}"));
            state.lifecycle = "reconciliation_required".into();
            state.save()?;
            Err(error).context("mutation outcome requires reconciliation")
        }
    }
}
