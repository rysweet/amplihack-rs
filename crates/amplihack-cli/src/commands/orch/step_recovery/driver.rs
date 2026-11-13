//! The `run` loop: run the step, and on failure ask the recovery agent what
//! to do, record its choice, and act on it. It reacts to events only: no
//! waits, no timers and no attempt counter.

use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

use super::exec::{self, Failed, Ran, Status};
use super::record::{
    self, Choice, Failure, Record, Source, check_text, validate_alternative_name,
    validate_caller_step_id,
};
use super::{RUN_REQUESTER, SCHEMA_VERSION, STEP_RECOVERY_HANDOFF_EXIT, evidence, recover};

pub const UNAVAILABLE: &str = "runner does not expose failed sub-recipe status";

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    #[arg(long)]
    pub step_id: String,
    #[arg(long)]
    pub dir: PathBuf,
    #[arg(long)]
    pub goal: String,
    #[arg(long)]
    pub state_file: PathBuf,
    /// Default handoff owner: a role or a value from the run context.
    #[arg(long)]
    pub owner: Option<String>,
    /// `<name>=<command>`; the command is split on whitespace, no shell.
    #[arg(long = "alternative")]
    pub alternatives: Vec<String>,
    /// The step must not be skipped: `continue_recorded` is not offered.
    #[arg(long)]
    pub essential: bool,
    /// Skip the first run and go straight to recovery.
    #[arg(long)]
    pub already_failed: bool,
    /// Run `<dir>/step-failure-recovery.yaml` instead of the bundle's.
    #[arg(long)]
    pub recipe_dir: Option<PathBuf>,
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

/// One thing the driver can run: the step itself or a declared alternative.
pub struct Attempt {
    pub step_id: String,
    pub command: Vec<String>,
    /// Declared alternatives; an alternative has none of its own.
    pub alternatives: Vec<(String, Vec<String>)>,
}

/// Checked settings shared by every pass of the loop.
pub struct Run {
    pub dir: PathBuf,
    pub goal: String,
    pub state_file: PathBuf,
    pub default_owner: String,
    pub essential: bool,
    pub recipe_dir: Option<PathBuf>,
}

fn parse_alternative(spec: &str) -> Result<(String, Vec<String>)> {
    let (name, command) = spec
        .split_once('=')
        .with_context(|| format!("--alternative must be <name>=<command>: {spec:?}"))?;
    validate_alternative_name(name)?;
    let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    ensure!(!argv.is_empty(), "--alternative {name} has no command");
    Ok((name.to_string(), argv))
}

pub fn absolute_dir(dir: &Path) -> Result<()> {
    ensure!(
        dir.is_absolute(),
        "the recovery directory must be an absolute path: {}",
        dir.display()
    );
    super::secure_fs::check_dir(dir)
        .with_context(|| format!("unsafe recovery directory {}", dir.display()))
}

/// Checks the arguments and runs the loop. Returns the exit code.
pub fn run(args: RunArgs) -> Result<i32> {
    validate_caller_step_id(&args.step_id)?;
    absolute_dir(&args.dir)?;
    let default_owner = match args.owner.as_deref().map(str::trim) {
        Some(owner) if !owner.is_empty() => {
            check_text("--owner", owner)?;
            owner.to_string()
        }
        _ => RUN_REQUESTER.to_string(),
    };
    let mut alternatives = Vec::new();
    for spec in &args.alternatives {
        let (name, argv) = parse_alternative(spec)?;
        ensure!(
            !alternatives
                .iter()
                .any(|(n, _): &(String, Vec<String>)| *n == name),
            "--alternative {name} is declared twice"
        );
        alternatives.push((name, argv));
    }
    let settings = Run {
        dir: args.dir,
        goal: args.goal,
        state_file: args.state_file,
        default_owner,
        essential: args.essential,
        recipe_dir: args.recipe_dir,
    };
    let mut attempt = Attempt {
        step_id: args.step_id,
        command: args.command,
        alternatives,
    };
    let mut pending = args.already_failed.then(|| Failed {
        status: Status::Unavailable,
        output_tail: String::new(),
    });
    loop {
        let failed = match pending.take() {
            Some(failed) => failed,
            None => match exec::execute(&settings.dir, &attempt.command)? {
                Ran::Success(spool) => {
                    exec::pass_through(spool)?;
                    return Ok(0);
                }
                Ran::Failed(failed) => failed,
            },
        };
        let failure = describe(&failed);
        match failed.status {
            Status::Exit(79) => {
                policy(
                    &settings,
                    &attempt,
                    failure,
                    "terminal policy refusal (exit 79)",
                    "review the policy refusal; the operation was refused on purpose",
                )?;
                return Ok(79);
            }
            Status::Exit(code) if code == STEP_RECOVERY_HANDOFF_EXIT => {
                eprintln!(
                    "[step-recovery] {} ended in a recorded handoff (exit {code}); passing it on",
                    attempt.step_id
                );
                return Ok(code);
            }
            Status::Signal(n) => {
                policy(
                    &settings,
                    &attempt,
                    failure,
                    &format!("terminated by signal {n}"),
                    "check why the step was stopped; rerun it if the run should continue",
                )?;
                return Ok(128 + n);
            }
            _ => {}
        }
        let (record, path) = recover::recover(&settings, &attempt, failure)?;
        match record.choice {
            Choice::RetryAfterRepair => {
                eprintln!(
                    "[step-recovery] {}: retrying after repair: {}",
                    attempt.step_id,
                    record.repair.as_deref().unwrap_or_default()
                );
            }
            Choice::AlternativeStep => {
                let name = record.alternative.clone().unwrap_or_default();
                let command = attempt
                    .alternatives
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, c)| c.clone())
                    .context("validated alternative is not declared")?;
                eprintln!(
                    "[step-recovery] {}: running alternative {name}",
                    attempt.step_id
                );
                attempt = Attempt {
                    step_id: format!("{}:alt:{name}", base_id(&attempt.step_id)),
                    command,
                    alternatives: Vec::new(),
                };
            }
            Choice::ContinueRecorded | Choice::Handoff => {
                print_outcome(&record, &path)?;
                return Ok(0);
            }
        }
    }
}

fn base_id(step_id: &str) -> &str {
    step_id
        .split_once(":alt:")
        .map_or(step_id, |(base, _)| base)
}

/// The record's failure block: redacted excerpt, status and signature.
pub fn describe(failed: &Failed) -> Failure {
    let (excerpt, tail) = evidence::excerpt_and_tail(&failed.output_tail);
    let class = super::signature::class_of(&tail);
    let (exit_code, signal, unavailable_reason) = match failed.status {
        Status::Exit(code) => (Some(code), None, None),
        Status::Signal(n) => (None, Some(n), None),
        Status::Unavailable => (None, None, Some(UNAVAILABLE.to_string())),
    };
    Failure {
        exit_code,
        signal,
        unavailable_reason,
        signature: super::signature::signature(exit_code, class, &tail),
        output_excerpt: excerpt,
    }
}

fn policy(run: &Run, attempt: &Attempt, failure: Failure, reason: &str, needs: &str) -> Result<()> {
    let record = Record {
        schema_version: SCHEMA_VERSION,
        step_id: attempt.step_id.clone(),
        failure,
        choice: Choice::Handoff,
        reason: reason.to_string(),
        repair: None,
        alternative: None,
        owner: Some(run.default_owner.clone()),
        needs: Some(needs.to_string()),
        recurrence: false,
        source: Source::Policy,
        recorded_at: record::now_utc(),
    };
    let path = record::append(&run.dir, &record)?;
    eprintln!(
        "[step-recovery] {}: {reason}; recorded in {}",
        attempt.step_id,
        path.display()
    );
    Ok(())
}

/// Prints the `{"step_recovery":…}` outcome line. Never the excerpt.
///
/// A plain text line comes first: the runner parses an output that is one
/// JSON object into an object, and `'step_recovery' in <output>` then never
/// matches. With the text line the output stays a string, so later steps'
/// documented `condition:` works.
pub fn print_outcome(record: &Record, path: &Path) -> Result<()> {
    println!(
        "step-recovery: {} recorded for {}",
        record.choice.as_str(),
        record.step_id
    );
    let mut inner = serde_json::json!({
        "choice": record.choice.as_str(),
        "step_id": record.step_id,
        "reason": record.reason,
        "source": record.source,
        "record": path.to_string_lossy(),
    });
    for (key, value) in [("owner", &record.owner), ("needs", &record.needs)] {
        if let Some(v) = value {
            inner[key] = serde_json::Value::String(v.clone());
        }
    }
    println!("{}", serde_json::json!({ "step_recovery": inner }));
    eprintln!(
        "[step-recovery] {}: {} ({:?}): {}; record {}",
        record.step_id,
        record.choice.as_str(),
        record.source,
        record.reason,
        path.display()
    );
    Ok(())
}
