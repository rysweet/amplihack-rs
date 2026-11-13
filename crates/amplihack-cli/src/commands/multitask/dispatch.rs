//! Dependency-aware dispatch using child completion and signal events.
use super::execution::{Children, Event, launch};
use super::{
    completion,
    models::WorkstreamConfig,
    plan,
    runtime::{self, Runtime},
    tracking,
};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};

use std::path::{Path, PathBuf};

use std::sync::mpsc;
use std::thread;

pub(super) fn lock() -> Result<File> {
    plan::private_directory(&runtime::base_dir())?;
    let path = runtime::base_dir().join("dispatch.lock");
    ensure!(!path.is_symlink(), "dispatch lock is a symlink");
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock_exclusive()
        .context("another dispatcher owns this runtime; stop it before resuming or reconciling")?;
    Ok(file)
}

pub(super) fn configuration(
    path: &Path,
    default_recipe: &str,
    roots: &BTreeMap<String, PathBuf>,
) -> Result<Vec<WorkstreamConfig>> {
    let raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    let array = raw.as_array().context("configuration must be an array")?;
    let mut leaves = Vec::new();
    for (index, value) in array.iter().enumerate() {
        let mut v = value.clone();
        if v.get("key").is_none() && v.get("name").is_none() {
            v["key"] = json!(format!("workstream-{}", index + 1));
        }
        if v.get("classification").is_none()
            && v.get("task_type").is_none()
            && v.get("type").is_none()
        {
            let recipe = v
                .get("recipe")
                .and_then(Value::as_str)
                .unwrap_or(default_recipe);
            v["classification"] = json!(match recipe {
                "operations-direct" => "Operations",
                "investigation-workflow" => "Investigation",
                "qa-workflow" => "Q&A",
                "consensus-workflow" => "Consensus",
                _ => "Development",
            });
        }
        leaves.push(v);
    }
    let mut input = json!({"workstreams": leaves});
    plan::allocate_identities(&mut input)?;
    let mut entries = plan::normalise_plan(&input, &plan::repository_names(roots), default_recipe)?;
    // A configuration is an operator-selected plan. Preserve a trusted Development
    // selection already made by the decomposition builder; never trust other names.
    for (entry, original) in entries.iter_mut().zip(array) {
        if entry.classification == "Development" {
            if let Some(recipe @ ("quick-change" | "default-workflow")) =
                original.get("recipe").and_then(Value::as_str)
            {
                entry.recipe = Some(recipe.into());
            }
        }
    }
    // Allocate legacy IDs only once, before any external action. Resume uses this file.
    if raw != serde_json::to_value(&entries)? {
        runtime::atomic_json(path, &entries)?;
    }
    Ok(entries)
}

fn prepare_workspace(state: &Runtime, repository: &Path) -> Result<()> {
    if state.config.classification == "Operations" || state.work_dir.exists() {
        return Ok(());
    }
    let mut cmd = amplihack_git::command();
    let output = cmd
        .args(["remote", "get-url", "origin"])
        .current_dir(repository)
        .output()?;
    ensure!(
        output.status.success(),
        "cannot resolve Development repository origin"
    );
    let remote = String::from_utf8(output.stdout)?;
    let output = amplihack_git::command()
        .args(["clone", "--depth=1", "--", remote.trim()])
        .arg(&state.work_dir)
        .current_dir(repository)
        .output()?;
    ensure!(
        output.status.success(),
        "Development clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn prerequisites(state: &Runtime, states: &[Runtime]) -> Result<Vec<Value>> {
    state
        .config
        .depends_on
        .iter()
        .map(|id| {
            let p = states
                .iter()
                .find(|s| s.id() == *id && s.lifecycle == "succeeded")
                .context("prerequisite has not durably succeeded")?;
            let result = p.result.as_ref().context("prerequisite result missing")?;
            completion::validate(result, &p.config)?;
            Ok(
                json!({"workstream_id": p.id(), "repository": p.config.repository,
            "result_revision": p.revision, "outcome": "succeeded", "outputs": result["outputs"]}),
            )
        })
        .collect()
}

fn result_from_output(state: &Runtime, stdout: &[u8]) -> Result<Value> {
    let path = state.artefact_dir.join("result.json");
    if path.exists() {
        runtime::owned(&path, &state.artefact_dir)?;
        return Ok(serde_json::from_slice(&fs::read(path)?)?);
    }
    let text = std::str::from_utf8(stdout).context("child output is not UTF-8")?;
    // Classic agents and recipe completion markers may include surrounding diagnostics.
    let result = crate::commands::orch::extract_json_with_field(text, "schema_version")
        .context("child exited without a completion result")?;
    Ok(result)
}

pub fn run(config_path: &Path, mode: &str, default_recipe: &str, dry_run: bool) -> Result<()> {
    ensure!(
        ["recipe", "classic"].contains(&mode),
        "unknown execution mode {mode:?}"
    );
    let _lock = lock()?;
    let roots = plan::repositories()?;
    let configs = configuration(config_path, default_recipe, &roots)?;
    if dry_run {
        for e in configs {
            println!(
                "{} {} {}",
                e.workstream_id.unwrap(),
                e.classification,
                e.recipe.unwrap()
            );
        }
        return Ok(());
    }
    plan::private_directory(&runtime::base_dir().join("state"))?;
    // Load and validate every record before creating issues, cloning or launching.
    let mut states: Vec<Runtime> = configs
        .into_iter()
        .map(|c| {
            let repo = roots
                .get(&c.repository)
                .context("unauthorised repository")?;
            Runtime::load(c, repo)
        })
        .collect::<Result<_>>()?;
    let mut paths = HashSet::new();
    for state in &states {
        ensure!(
            paths.insert(state.state_file.clone()),
            "ambiguous legacy adoption: two leaves own the same state file"
        );
    }
    for state in &mut states {
        if ["running", "tracking"].contains(&state.lifecycle.as_str()) {
            state.lifecycle = "reconciliation_required".into();
            state.diagnostic =
                "interrupted execution; inspect external outcomes before resuming".into();
            state.save()?;
        }
        if state.lifecycle == "reconciliation_required" {
            bail!(
                "{} requires reconciliation: {}",
                state.id(),
                state.diagnostic
            );
        }
        state.save()?;
    }
    let concurrency = super::waves::wave_size(states.len(), super::waves::concurrency_limit());
    let (tx, rx) = mpsc::channel();
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])?;
    let handle = signals.handle();
    let signal_tx = tx.clone();
    let signal_waiter = thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = signal_tx.send(Event::Interrupted);
        }
    });
    // Closing the signal iterator on every return also joins its waiter.
    struct SignalGuard(
        signal_hook::iterator::Handle,
        Option<thread::JoinHandle<()>>,
    );
    impl Drop for SignalGuard {
        fn drop(&mut self) {
            self.0.close();
            if let Some(t) = self.1.take() {
                if t.join().is_err() {
                    tracing::error!("signal waiter panicked");
                }
            }
        }
    }
    let _signals = SignalGuard(handle, Some(signal_waiter));
    let mut active = Children(HashMap::new());
    let mut interrupted = false;
    loop {
        let failed: HashSet<_> = states
            .iter()
            .filter(|s| {
                ["failed", "blocked", "reconciliation_required", "abandoned"]
                    .contains(&s.lifecycle.as_str())
            })
            .map(Runtime::id)
            .collect();
        for state in &mut states {
            if ["pending", "ready", "blocked"].contains(&state.lifecycle.as_str()) {
                let blocked: Vec<_> = state
                    .config
                    .depends_on
                    .iter()
                    .filter(|id| failed.contains(id))
                    .copied()
                    .collect();
                if !blocked.is_empty() {
                    state.lifecycle = "blocked".into();
                    state.blocked_by = blocked;
                    state.save()?;
                } else if state.lifecycle == "blocked" {
                    state.lifecycle = "pending".into();
                    state.blocked_by.clear();
                    state.save()?;
                }
            }
        }
        let succeeded: HashSet<_> = states
            .iter()
            .filter(|s| s.lifecycle == "succeeded")
            .map(Runtime::id)
            .collect();
        for index in 0..states.len() {
            if interrupted || active.0.len() >= concurrency {
                break;
            }
            if !["pending", "ready"].contains(&states[index].lifecycle.as_str())
                || !states[index]
                    .config
                    .depends_on
                    .iter()
                    .all(|id| succeeded.contains(id))
            {
                continue;
            }
            let prerequisites = prerequisites(&states[index], &states)?;
            let state = &mut states[index];
            let attempt = (|| {
                // Required tracking is resolved in the trusted repository, before a Development clone exists.
                let repo = &roots[&state.config.repository];
                if state.config.tracking.required && state.config.issue.is_null() {
                    let working = state.work_dir.clone();
                    state.work_dir = repo.clone();
                    let resolved = tracking::resolve(state, &prerequisites);
                    state.work_dir = working;
                    resolved?;
                    state.save()?;
                }
                prepare_workspace(state, repo)?;
                launch(state, mode, &prerequisites, tx.clone())
            })();
            match attempt {
                Ok(child) => {
                    active.0.insert(state.id(), child);
                }
                Err(error) => {
                    if state.lifecycle != "reconciliation_required" {
                        state.lifecycle = "failed".into();
                    }
                    state.diagnostic = format!("{error:#}");
                    state.save()?;
                    eprintln!("{}: {}", state.id(), state.diagnostic);
                }
            }
        }
        if active.0.is_empty() {
            if states.iter().all(|s| s.lifecycle == "succeeded") {
                println!(
                    "{}",
                    json!({"outcome": "succeeded", "results": states.iter().map(|s| json!({"workstream_id": s.id(), "repository": s.config.repository, "result_revision": s.revision, "result": s.result})).collect::<Vec<_>>()})
                );
                return Ok(());
            }
            // One pass propagates each failed edge; keep going until all pending descendants are blocked.
            if states
                .iter()
                .any(|s| s.lifecycle == "pending" || s.lifecycle == "ready")
                && !interrupted
            {
                let terminal: HashSet<_> = states
                    .iter()
                    .filter(|s| s.lifecycle != "pending" && s.lifecycle != "ready")
                    .map(Runtime::id)
                    .collect();
                if states.iter().any(|s| {
                    s.config.depends_on.iter().any(|id| terminal.contains(id))
                        && (s.lifecycle == "pending" || s.lifecycle == "ready")
                }) {
                    continue;
                }
            }
            bail!(
                "workstream execution incomplete: {}",
                states
                    .iter()
                    .filter(|s| s.lifecycle != "succeeded")
                    .map(|s| format!("{} {} {}", s.id(), s.lifecycle, s.diagnostic))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        match rx.recv().context("child completion channel closed")? {
            Event::Interrupted => {
                interrupted = true;
                for state in &mut states {
                    if active.0.contains_key(&state.id()) {
                        state.lifecycle = "reconciliation_required".into();
                        state.diagnostic = "interrupted; reconcile before replay".into();
                        state.save()?;
                    }
                }
                for child in active.0.values() {
                    child.cancel()?;
                }
            }
            Event::Completed(id, exit) => {
                active
                    .0
                    .remove(&id)
                    .context("completion for an unowned child")?;
                let state = states
                    .iter_mut()
                    .find(|s| s.id() == id)
                    .context("unknown completion identity")?;
                let completed = (|| {
                    let (code, bytes) = exit?;
                    let durable: Runtime = serde_json::from_slice(&fs::read(&state.state_file)?)?;
                    ensure!(
                        durable.id() == state.id()
                            && durable.config.repository == state.config.repository,
                        "child changed state ownership"
                    );
                    state.revision = durable.revision;
                    state.operation_attempts = durable.operation_attempts;
                    if durable.lifecycle == "reconciliation_required" {
                        state.lifecycle = durable.lifecycle;
                        bail!("an external mutation requires reconciliation");
                    }
                    state.last_exit_code = Some(code);
                    ensure!(code == 0, "child exited with status {code}");
                    let result = result_from_output(state, &bytes)?;
                    completion::validate(&result, &state.config)?;
                    for attempt in &state.operation_attempts {
                        if attempt["status"] == "not_applied" {
                            continue;
                        }
                        ensure!(
                            attempt["status"] == "succeeded",
                            "unresolved operation attempt"
                        );
                        let name = attempt["output_name"]
                            .as_str()
                            .context("operation output name")?;
                        ensure!(
                            result["outputs"][name] == attempt["output"],
                            "completion does not match the durable operation receipt"
                        );
                    }
                    state.lifecycle = result["outcome"].as_str().unwrap().into();
                    state.result = Some(result);
                    Ok(())
                })();
                if let Err(error) = completed {
                    if state.lifecycle != "reconciliation_required" {
                        state.lifecycle = "failed".into();
                    }
                    state.diagnostic = format!("{error:#}");
                    eprintln!("{id}: {}", state.diagnostic);
                }
                state.save()?;
            }
        }
    }
}
