//! One recovery pass: compute recurrence, ask the agent (unless the run must
//! not start one), validate its reply and append exactly one record.

use anyhow::{Context as _, Result, ensure};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::decision::{self, Context, Decision, Refusal, refuse};
use super::driver::{Attempt, Run};
use super::evidence::Evidence;
use super::invoke::{self, ACTIVE_ENV};
use super::record::{
    self, Choice, Failure, Record, Source, record_path, validate_alternative_name,
};
use super::{RUN_REQUESTER, SCHEMA_VERSION, secure_fs};

/// Parses `--alternative <name>=<command>` flags: each command is split on
/// whitespace (no shell), and a name may be declared only once.
pub fn parse_alternatives(specs: &[String]) -> Result<Vec<(String, Vec<String>)>> {
    let mut alternatives: Vec<(String, Vec<String>)> = Vec::new();
    for spec in specs {
        let (name, command) = spec
            .split_once('=')
            .with_context(|| format!("--alternative must be <name>=<command>: {spec:?}"))?;
        validate_alternative_name(name)?;
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        ensure!(!argv.is_empty(), "--alternative {name} has no command");
        ensure!(
            !alternatives.iter().any(|(n, _)| n == name),
            "--alternative {name} is declared twice"
        );
        alternatives.push((name.to_string(), argv));
    }
    Ok(alternatives)
}

/// Length and SHA-256 of a record file, or `None` when it does not exist.
fn fingerprint(path: &Path) -> Result<Option<(usize, Vec<u8>)>> {
    Ok(secure_fs::read_optional(path, record::RECORD_FILE_LIMIT)?
        .map(|bytes| (bytes.len(), Sha256::digest(&bytes).to_vec())))
}

/// The choices on offer for this failure. A guard step checks something,
/// so it may only report: no repair, and never a skip (`guard` implies
/// `essential`).
pub fn allowed_choices(
    has_alternatives: bool,
    essential: bool,
    guard: bool,
    recurrence: bool,
) -> Vec<Choice> {
    Choice::ALL
        .into_iter()
        .filter(|c| match c {
            Choice::AlternativeStep => has_alternatives,
            Choice::ContinueRecorded => !essential && !guard,
            Choice::RetryAfterRepair => !recurrence && !guard,
            Choice::Handoff => true,
        })
        .collect()
}

/// A failure is a recurrence when an earlier record of the same step has the
/// same signature and chose a retry or an alternative.
pub fn is_recurrence(prior: &[Record], signature: &str) -> bool {
    prior.iter().any(|r| {
        r.failure.signature == signature
            && matches!(r.choice, Choice::RetryAfterRepair | Choice::AlternativeStep)
    })
}

/// An earlier handoff anywhere in the run, as `(step id, record file)`.
fn earlier_handoff(dir: &Path) -> Result<Option<(String, PathBuf)>> {
    for (path, records) in record::read_all(dir)? {
        if let Some(r) = records.iter().find(|r| r.choice == Choice::Handoff) {
            return Ok(Some((r.step_id.clone(), path)));
        }
    }
    Ok(None)
}

/// Runs one recovery pass and returns the appended record and its file.
pub fn recover(run: &Run, attempt: &Attempt, failure: Failure) -> Result<(Record, PathBuf)> {
    let prior = record::read_step(&run.dir, &attempt.step_id)?;
    let recurrence = is_recurrence(&prior, &failure.signature);
    let names: Vec<String> = attempt
        .alternatives
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let allowed = allowed_choices(!names.is_empty(), run.essential, run.guard, recurrence);
    let trusted = vec![RUN_REQUESTER.to_string(), run.default_owner.clone()];
    let handed_off = earlier_handoff(&run.dir)?;

    let outcome: Result<Decision, Refusal> = if std::env::var_os(ACTIVE_ENV).is_some() {
        Err(refuse(
            format!(
                "{ACTIVE_ENV} is set: recovery is already running, so no recovery agent was started"
            ),
            None,
        ))
    } else if let Some((step, path)) = handed_off {
        Err(refuse(
            format!(
                "the run is already handed off at step {step} ({}); no recovery agent was started",
                path.display()
            ),
            None,
        ))
    } else {
        let file = Evidence {
            step_id: &attempt.step_id,
            goal: &run.goal,
            record_failure: &failure,
            allowed: &allowed,
            alternatives: &names,
            prior: &prior,
            recurrence,
            default_owner: &run.default_owner,
            guard: run.guard,
        }
        .write(&run.dir)?;
        let records = record_path(&run.dir, &attempt.step_id);
        let before = fingerprint(&records)?;
        let text = invoke::run_agent(&file, run.recipe_dir.as_deref());
        let after = fingerprint(&records)?;
        match text {
            Err(e) => Err(refuse(
                format!("check 1: the recovery agent could not run ({e:#})"),
                None,
            )),
            Ok(_) if before != after => Err(refuse(
                format!(
                    "the step's record file {} changed while the recovery agent ran",
                    records.display()
                ),
                None,
            )),
            Ok(text) => decision::validate(
                decision::find_reply(&text),
                &Context {
                    allowed: &allowed,
                    alternatives: &names,
                    trusted_owners: &trusted,
                    recurrence,
                    prior: &prior,
                    signature: &failure.signature,
                },
            ),
        }
    };

    let (decision, source) = match outcome {
        Ok(decision) => (decision, Source::Agent),
        Err(refused) => (
            Decision {
                choice: Choice::Handoff,
                reason: decision::refusal_reason(&refused),
                repair: None,
                alternative: None,
                owner: Some(run.default_owner.clone()),
                needs: Some(
                    "read the recovery record and the failed step's output, then decide how the run continues"
                        .to_string(),
                ),
            },
            Source::Validator,
        ),
    };
    let record = Record {
        schema_version: SCHEMA_VERSION,
        step_id: attempt.step_id.clone(),
        failure,
        choice: decision.choice,
        reason: decision.reason,
        repair: decision.repair,
        alternative: decision.alternative,
        owner: decision.owner,
        needs: decision.needs,
        recurrence,
        source,
        recorded_at: record::now_utc(),
    };
    let path = record::append(&run.dir, &record)?;
    Ok((record, path))
}
