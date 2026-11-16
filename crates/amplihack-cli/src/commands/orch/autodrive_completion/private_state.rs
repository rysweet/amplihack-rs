use super::{Prepare, evaluator, evidence, secure_fs, secure_fs::Scope};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub run_id: String,
    pub round_label: String,
    pub state_dir: PathBuf,
    pub record: PathBuf,
    pub repo: PathBuf,
    pub review_dir: PathBuf,
    pub launch_tools: PathBuf,
    pub binary: PathBuf,
    pub decision_file: PathBuf,
    pub context: BTreeMap<String, String>,
    pub review_stamp: String,
    pub depth: Option<String>,
    pub ceiling: Option<String>,
    /// The PR whose head the dispatch compares (#2333). 0 is treated as none.
    #[serde(default)]
    pub pr: Option<u64>,
}

/// Reads the private `state.json`. The state folder is not known yet, so the
/// check is strict: `prepare` has already made the state folder private, and
/// everything under `.completion-*` is created at 0700/0600. A folder with
/// extra bits is refused; a file this user owns with extra bits inside a
/// private (0700) folder is repaired to 0600 and logged (#2546).
pub fn load(path: &Path) -> Result<State> {
    Ok(serde_json::from_slice(&secure_fs::read(
        Scope::Strict,
        path,
        evidence::LIMIT,
    )?)?)
}
pub fn snapshot(path: &Path) -> Result<evidence::Snapshot> {
    Ok(serde_json::from_slice(&secure_fs::read(
        Scope::Strict,
        &path.with_extension("evidence"),
        evidence::LIMIT,
    )?)?)
}

pub fn seal(path: &Path) -> Result<()> {
    let state = load(path)?;
    let snapshot = evidence::capture(&state)?;
    secure_fs::create(
        Scope::of(&state.state_dir),
        &path.with_extension("evidence"),
        &serde_json::to_vec(&snapshot)?,
    )
}

/// A `--context-from-file` or `--evaluator-context-file` path must be inside
/// the state folder (#2546, S1): absolute, with no `..` part.
fn context_path(state_dir: &Path, path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path.components().any(|c| matches!(c, Component::ParentDir))
            && path.starts_with(state_dir),
        "context file must be inside the state directory: {}",
        path.display()
    );
    Ok(())
}

pub fn prepare(args: Prepare) -> Result<()> {
    ensure!(
        args.round_label.starts_with("round-") && args.round_label[6..].parse::<u64>().is_ok(),
        "invalid round label"
    );
    ensure!(
        args.state_dir.is_absolute()
            && args.record
                == args
                    .state_dir
                    .join(format!("crusty-{}.json", args.round_label)),
        "record must be the designated round in the state directory"
    );
    ensure!(
        !args
            .state_dir
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "state directory contains traversal"
    );
    // Only now, with the arguments valid, may the state folder be repaired.
    secure_fs::check_dir(&args.state_dir)?;
    let scope = Scope::of(&args.state_dir);
    let mut context: BTreeMap<String, String> = if let Some(path) = args.evaluator_context_file {
        context_path(&args.state_dir, &path)?;
        serde_json::from_slice(&secure_fs::read(scope, &path, evidence::LIMIT)?)?
    } else {
        BTreeMap::new()
    };
    for input in args.context_files {
        let (key, file) = input
            .split_once('=')
            .context("context-from-file requires key=path")?;
        ensure!(
            evaluator::FIELDS.contains(&key),
            "unknown evaluator context field"
        );
        ensure!(
            !context.contains_key(key),
            "duplicate evaluator context field"
        );
        context_path(&args.state_dir, Path::new(file))?;
        context.insert(
            key.to_string(),
            String::from_utf8(secure_fs::read(scope, Path::new(file), evidence::LIMIT)?)?,
        );
    }
    evaluator::validate_context(&context)?;
    let launch_tools = args
        .launch_tools
        .canonicalize()
        .context("launch tools missing")?;
    ensure!(
        launch_tools
            .join("../recipes/loop-health-evaluator.yaml")
            .is_file(),
        "trusted evaluator recipe missing"
    );
    // Request owner-only access at creation; tempfile's default directory mode
    // is 0777, filtered by the caller's umask. Children separate authority/output.
    let mut builder = tempfile::Builder::new();
    builder.prefix(".completion-");
    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));
    let root = builder.tempdir_in(&args.state_dir)?.keep();
    let trusted = root.join("driver");
    let output = root.join("output");
    for path in [&trusted, &output] {
        #[cfg(unix)]
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
        #[cfg(not(unix))]
        std::fs::create_dir(path)?;
    }
    let private_state = trusted.join("state.json");
    let decision_file = output.join("decision.json");
    let run_id = uuid::Uuid::new_v4().to_string();
    let binary = std::env::var_os("AMPLIHACK_BIN")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    let binary = if binary.is_absolute() {
        binary
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(&binary))
            .find(|p| p.is_file())
            .context("launch binary not found")?
    };
    let state = State {
        review_stamp: evidence::review_stamp(&launch_tools, &args.review_dir)?,
        run_id: run_id.clone(),
        round_label: args.round_label,
        state_dir: args.state_dir,
        record: args.record,
        repo: args.repo.canonicalize()?,
        review_dir: args.review_dir.canonicalize()?,
        launch_tools,
        binary,
        decision_file: decision_file.clone(),
        context,
        depth: std::env::var("AMPLIHACK_SESSION_DEPTH").ok(),
        ceiling: std::env::var("AMPLIHACK_MAX_DEPTH").ok(),
        pr: args.pr,
    };
    secure_fs::create(
        Scope::of(&state.state_dir),
        &private_state,
        &serde_json::to_vec(&state)?,
    )?;
    // Direct dispatch clients may prepare existing evidence. In the loop the
    // record step seals fresh evidence before the decision agent is invoked.
    if state.record.exists() {
        seal(&private_state)?;
    }
    println!(
        "{}",
        serde_json::json!({"run_id":run_id,"private_state":private_state,"decision_file":decision_file})
    );
    Ok(())
}
