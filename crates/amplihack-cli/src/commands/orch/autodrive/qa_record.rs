//! Criterion 1's record (#2569). The judging agent step decides whether a PR
//! changes user-facing behaviour, runs or writes the outside-in scenarios,
//! and loops on failures until PASS or HANDOFF. This module is the only way
//! its judgement reaches `qa-evidence.json`.
//!
//! It judges nothing. It checks the record's shape (the type constraints that
//! replace the shell status mapping deleted from `autodrive_qa_evidence.sh`),
//! adds the facts the agent may not supply (`head_sha` from the repository,
//! `qa_reason` from the status, the declared command from `qa-inputs.json`),
//! and writes the file atomically and privately. It never opens a cited or
//! scenario path and never compares them with the diff.
//!
//! Exit codes: 0 written, 2 the judgement was refused and nothing was written,
//! 1 an environment error (inputs, repository or output directory).
use super::trusted;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Largest judgement or inputs file accepted.
pub const MAX_INPUT_BYTES: usize = 1 << 20;
const MAX_STRING_BYTES: usize = 4 * 1024;
const MAX_REASON_BYTES: usize = 16 * 1024;
const MAX_LIST_ENTRIES: usize = 1000;

/// A judgement the record contract does not accept. Displayed as
/// `refused: <rule> at <field>`; the offending value is never repeated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub rule: &'static str,
    pub field: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused: {} at {}", self.rule, self.field)
    }
}

impl std::error::Error for Refusal {}

fn refuse<T>(rule: &'static str, field: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal {
        rule,
        field: field.into(),
    })
}

fn schema<T>(field: impl Into<String>) -> Result<T, Refusal> {
    refuse("schema", field)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum QaStatus {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "HANDOFF")]
    Handoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Judgement {
    UserFacing,
    NotUserFacing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    Product,
    Scenario,
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scenario {
    pub path: String,
    pub written_in_pr: bool,
    pub commit: Option<String>,
    pub result: RunResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Iteration {
    pub attempt: u64,
    /// `None` means the attempt was a run of the declared QA command.
    pub failing_scenario: Option<String>,
    pub cause: Cause,
    pub fix_commit: Option<String>,
    pub result: RunResult,
}

/// Field names match the arguments of `step-recovery record` (#2539, #2570).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Handoff {
    pub step_id: String,
    pub failure: String,
    pub choice: &'static str,
    pub reason: String,
    pub owner: String,
    pub needs: String,
}

/// The agent's judgement, as accepted from stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgementRecord {
    pub qa_status: QaStatus,
    pub qa_judgement: Judgement,
    pub qa_judgement_reason: String,
    pub qa_cited_files: Vec<String>,
    pub qa_scenarios: Vec<Scenario>,
    pub qa_iterations: Vec<Iteration>,
    pub qa_handoff: Option<Handoff>,
}

const TOP_KEYS: [&str; 7] = [
    "qa_status",
    "qa_judgement",
    "qa_judgement_reason",
    "qa_cited_files",
    "qa_scenarios",
    "qa_iterations",
    "qa_handoff",
];
const SCENARIO_KEYS: [&str; 4] = ["path", "written_in_pr", "commit", "result"];
const ITERATION_KEYS: [&str; 5] = [
    "attempt",
    "failing_scenario",
    "cause",
    "fix_commit",
    "result",
];
const HANDOFF_KEYS: [&str; 6] = ["step_id", "failure", "choice", "reason", "owner", "needs"];

/// A key name is echoed only when it is a plain identifier; anything else
/// could carry the agent's text into the refusal line.
fn key_label(prefix: &str, key: &str) -> String {
    let plain = !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    let key = if plain { key } else { "<unknown-key>" };
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

/// Every key present, none unknown: the record has no optional fields.
fn exact_object<'a>(
    v: &'a Value,
    keys: &[&str],
    path: &str,
) -> Result<&'a Map<String, Value>, Refusal> {
    let Some(map) = v.as_object() else {
        return schema(if path.is_empty() { "$" } else { path });
    };
    if let Some(unknown) = map.keys().find(|k| !keys.contains(&k.as_str())) {
        return schema(key_label(path, unknown));
    }
    if let Some(missing) = keys.iter().find(|k| !map.contains_key(**k)) {
        return schema(key_label(path, missing));
    }
    Ok(map)
}

/// One line, no control characters (C0, DEL, C1, so no ANSI escapes), and at
/// most `limit` bytes.
fn check_str(s: &str, limit: usize, path: &str) -> Result<(), Refusal> {
    if s.len() <= limit && !s.chars().any(char::is_control) {
        Ok(())
    } else {
        schema(path)
    }
}

fn text(v: &Value, limit: usize, path: &str) -> Result<String, Refusal> {
    match v.as_str() {
        Some(s) => check_str(s, limit, path).map(|()| s.to_owned()),
        None => schema(path),
    }
}

/// A relative path with no `..` component. It is never opened.
fn rel_path(v: &Value, path: &str) -> Result<String, Refusal> {
    let s = text(v, MAX_STRING_BYTES, path)?;
    let p = Path::new(&s);
    let ok = !s.is_empty()
        && !s.starts_with('/')
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if ok { Ok(s) } else { schema(path) }
}

fn commit_id(v: &Value, path: &str) -> Result<Option<String>, Refusal> {
    match v {
        Value::Null => Ok(None),
        Value::String(s)
            if (7..=64).contains(&s.len())
                && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            Ok(Some(s.clone()))
        }
        _ => schema(path),
    }
}

fn list<'a>(v: &'a Value, path: &str) -> Result<&'a Vec<Value>, Refusal> {
    match v.as_array() {
        Some(a) if a.len() <= MAX_LIST_ENTRIES => Ok(a),
        _ => schema(path),
    }
}

fn result(v: &Value, path: &str) -> Result<RunResult, Refusal> {
    match v.as_str() {
        Some("pass") => Ok(RunResult::Pass),
        Some("fail") => Ok(RunResult::Fail),
        _ => schema(path),
    }
}

fn parse(v: &Value) -> Result<JudgementRecord, Refusal> {
    let m = exact_object(v, &TOP_KEYS, "")?;
    let qa_status = match m["qa_status"].as_str() {
        Some("PASS") => QaStatus::Pass,
        Some("FAIL") => QaStatus::Fail,
        Some("HANDOFF") => QaStatus::Handoff,
        _ => return schema("qa_status"),
    };
    let qa_judgement = match m["qa_judgement"].as_str() {
        Some("user_facing") => Judgement::UserFacing,
        Some("not_user_facing") => Judgement::NotUserFacing,
        _ => return schema("qa_judgement"),
    };
    let qa_judgement_reason = text(
        &m["qa_judgement_reason"],
        MAX_REASON_BYTES,
        "qa_judgement_reason",
    )?;
    let qa_cited_files = list(&m["qa_cited_files"], "qa_cited_files")?
        .iter()
        .enumerate()
        .map(|(i, f)| rel_path(f, &format!("qa_cited_files[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut qa_scenarios = Vec::new();
    for (i, s) in list(&m["qa_scenarios"], "qa_scenarios")?.iter().enumerate() {
        let p = format!("qa_scenarios[{i}]");
        let s = exact_object(s, &SCENARIO_KEYS, &p)?;
        qa_scenarios.push(Scenario {
            path: rel_path(&s["path"], &format!("{p}.path"))?,
            written_in_pr: s["written_in_pr"]
                .as_bool()
                .map_or_else(|| schema(format!("{p}.written_in_pr")), Ok)?,
            commit: commit_id(&s["commit"], &format!("{p}.commit"))?,
            result: result(&s["result"], &format!("{p}.result"))?,
        });
    }
    let mut qa_iterations = Vec::new();
    for (i, it) in list(&m["qa_iterations"], "qa_iterations")?
        .iter()
        .enumerate()
    {
        let p = format!("qa_iterations[{i}]");
        let it = exact_object(it, &ITERATION_KEYS, &p)?;
        let attempt = match it["attempt"].as_u64() {
            Some(a) if a >= 1 => a,
            _ => return schema(format!("{p}.attempt")),
        };
        let failing_scenario = match &it["failing_scenario"] {
            Value::Null => None,
            other => Some(rel_path(other, &format!("{p}.failing_scenario"))?),
        };
        let cause = match it["cause"].as_str() {
            Some("product") => Cause::Product,
            Some("scenario") => Cause::Scenario,
            Some("environment") => Cause::Environment,
            _ => return schema(format!("{p}.cause")),
        };
        qa_iterations.push(Iteration {
            attempt,
            failing_scenario,
            cause,
            fix_commit: commit_id(&it["fix_commit"], &format!("{p}.fix_commit"))?,
            result: result(&it["result"], &format!("{p}.result"))?,
        });
    }
    let qa_handoff = match &m["qa_handoff"] {
        Value::Null => None,
        h => {
            let h = exact_object(h, &HANDOFF_KEYS, "qa_handoff")?;
            let part = |k: &str| text(&h[k], MAX_STRING_BYTES, &format!("qa_handoff.{k}"));
            if h["choice"].as_str() != Some("handoff") {
                return schema("qa_handoff.choice");
            }
            Some(Handoff {
                step_id: part("step_id")?,
                failure: part("failure")?,
                choice: "handoff",
                reason: part("reason")?,
                owner: part("owner")?,
                needs: part("needs")?,
            })
        }
    };
    Ok(JudgementRecord {
        qa_status,
        qa_judgement,
        qa_judgement_reason,
        qa_cited_files,
        qa_scenarios,
        qa_iterations,
        qa_handoff,
    })
}

/// The shape rules: what each status and judgement must carry.
fn shape(j: &JudgementRecord) -> Result<(), Refusal> {
    if j.qa_judgement_reason.trim().is_empty() {
        return refuse("reason-empty", "qa_judgement_reason");
    }
    match (&j.qa_handoff, j.qa_status) {
        (Some(_), QaStatus::Pass | QaStatus::Fail) => {
            return refuse("handoff-without-status", "qa_handoff");
        }
        (None, QaStatus::Handoff) => return refuse("handoff-incomplete", "qa_handoff"),
        (Some(h), QaStatus::Handoff) => {
            for (k, v) in [
                ("step_id", &h.step_id),
                ("failure", &h.failure),
                ("reason", &h.reason),
                ("owner", &h.owner),
                ("needs", &h.needs),
            ] {
                if v.trim().is_empty() {
                    return refuse("handoff-incomplete", format!("qa_handoff.{k}"));
                }
            }
        }
        (None, _) => {}
    }
    if j.qa_status == QaStatus::Pass {
        match j.qa_judgement {
            Judgement::NotUserFacing if j.qa_cited_files.is_empty() => {
                return refuse("not-user-facing-needs-cited-files", "qa_cited_files");
            }
            // A passing declared command is not a scenario and does not count.
            Judgement::UserFacing
                if !j.qa_scenarios.iter().any(|s| s.result == RunResult::Pass) =>
            {
                return refuse("user-facing-needs-passing-scenario", "qa_scenarios");
            }
            _ => {}
        }
    }
    for (i, it) in j.qa_iterations.iter().enumerate() {
        if it.attempt != i as u64 + 1 {
            return refuse("iteration-order", format!("qa_iterations[{i}].attempt"));
        }
    }
    Ok(())
}

/// Check a judgement read from stdin. Pure: no I/O.
pub fn check_judgement(bytes: &[u8]) -> Result<JudgementRecord, Refusal> {
    if bytes.len() > MAX_INPUT_BYTES {
        return refuse("input-too-large", "stdin");
    }
    let v: Value = serde_json::from_slice(bytes).or_else(|_| schema("$"))?;
    let j = parse(&v)?;
    shape(&j)?;
    Ok(j)
}

/// `qa_reason`, derived from the status for readers of the legacy field.
fn derived_reason(status: QaStatus) -> &'static str {
    match status {
        QaStatus::Pass => "",
        QaStatus::Fail => "qa-scenario-failed",
        QaStatus::Handoff => "qa-handoff",
    }
}

/// The declared QA command as `autodrive_qa_evidence.sh` measured it. #2458
/// adds its kind and scenarios fields here and in that script's block. Unknown
/// fields are ignored (R2.2), so a newer script does not break an older
/// binary; only the fields named here are copied.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DeclaredQa {
    pub source: String,
    pub dir: String,
    pub command: String,
    pub status: String,
    pub exit_status: Option<i64>,
    pub cause: String,
    pub candidates: String,
    pub ci_skip: String,
    /// Read so the field is known; never written to the evidence, because test
    /// output can hold secrets.
    #[serde(skip_serializing)]
    pub log_tail: String,
}

/// `qa-inputs.json`, written by `autodrive_qa_evidence.sh` (step-02a). The
/// agent reads every field; this module copies only the declared command and
/// the flat legacy facts. Every documented field is required; unknown fields
/// are ignored (R2.2).
// Fields are parsed so the file's shape is checked; only some are copied.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct QaInputs {
    pub schema: u32,
    pub qa_round: String,
    pub head_sha: String,
    pub base_sha: String,
    pub repo_type: String,
    pub docs_only: String,
    pub changed_files: Vec<String>,
    pub scenario_dir: String,
    pub existing_scenarios: Vec<String>,
    pub declared_qa: DeclaredQa,
    pub git_env_cleared: String,
    pub run_env_cleared: String,
}

#[derive(Debug, Serialize)]
struct DeclaredCommand<'a> {
    source: &'a str,
    dir: &'a str,
    command: &'a str,
    exit_status: Option<i64>,
}

/// The evidence file. `qa_status` is what the gate reads; the flat legacy
/// fields keep their names for readers of earlier records.
#[derive(Debug, Serialize)]
struct Evidence<'a> {
    qa_status: QaStatus,
    qa_reason: &'static str,
    qa_judgement: Judgement,
    qa_judgement_reason: &'a str,
    qa_cited_files: &'a [String],
    qa_scenarios: &'a [Scenario],
    qa_iterations: &'a [Iteration],
    qa_handoff: &'a Option<Handoff>,
    qa_declared_command: DeclaredCommand<'a>,
    qa_round: &'a str,
    head_sha: &'a str,
    docs_only: &'a str,
    qa_repo_type: &'a str,
    qa_command: &'a str,
    qa_command_source: &'a str,
    qa_dir: &'a str,
    /// The declared command's exit status as a string, `""` when nothing ran.
    qa_exit_code: String,
    declared_qa: &'a DeclaredQa,
    git_env_cleared: &'a str,
    run_env_cleared: &'a str,
}

fn read_inputs(path: &Path, round: Option<&str>) -> Result<QaInputs> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open qa inputs {} without following links", path.display()))?;
    let meta = file.metadata().context("stat qa inputs")?;
    ensure!(meta.is_file(), "qa inputs is not a regular file");
    ensure!(
        meta.len() <= MAX_INPUT_BYTES as u64,
        "qa inputs exceeds 1 MiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("read qa inputs")?;
    ensure!(bytes.len() <= MAX_INPUT_BYTES, "qa inputs exceeds 1 MiB");
    let inputs: QaInputs = serde_json::from_slice(&bytes).map_err(|e| {
        anyhow::anyhow!("qa inputs is not the documented shape (line {})", e.line())
    })?;
    ensure!(
        inputs.schema == 1,
        "qa inputs has an unknown schema version"
    );
    check_copied(&inputs, round.is_none()).context("qa inputs")?;
    Ok(inputs)
}

/// Every inputs string copied into the evidence follows the judgement's text
/// rule (R2.3): the file sits in a directory the agent can write, so its
/// fields are no more trusted than stdin. `qa_round` is copied only when
/// `--round` is absent.
fn check_copied(inputs: &QaInputs, copies_round: bool) -> Result<(), Refusal> {
    let d = &inputs.declared_qa;
    let mut fields = vec![
        ("inputs.repo_type", inputs.repo_type.as_str()),
        ("inputs.docs_only", inputs.docs_only.as_str()),
        ("inputs.git_env_cleared", inputs.git_env_cleared.as_str()),
        ("inputs.run_env_cleared", inputs.run_env_cleared.as_str()),
        ("inputs.declared_qa.source", d.source.as_str()),
        ("inputs.declared_qa.dir", d.dir.as_str()),
        ("inputs.declared_qa.command", d.command.as_str()),
        ("inputs.declared_qa.status", d.status.as_str()),
        ("inputs.declared_qa.cause", d.cause.as_str()),
        ("inputs.declared_qa.candidates", d.candidates.as_str()),
        ("inputs.declared_qa.ci_skip", d.ci_skip.as_str()),
    ];
    if copies_round {
        fields.push(("inputs.qa_round", inputs.qa_round.as_str()));
    }
    for (path, s) in fields {
        check_str(s, MAX_STRING_BYTES, path)?;
    }
    Ok(())
}

/// The output's parent must be a private state directory: owned by this
/// user with no group or other access, the same check the evidence store
/// applies (`state::private`).
fn check_parent(out: &Path) -> Result<PathBuf> {
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_owned(),
        _ => PathBuf::from("."),
    };
    let name = out.file_name().context("--out names no file")?;
    ensure!(
        !Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_))),
        "--out names no file"
    );
    let meta = std::fs::metadata(&parent)
        .with_context(|| format!("stat output directory {}", parent.display()))?;
    super::state::private(&meta, true)
        .with_context(|| format!("output directory {}", parent.display()))?;
    Ok(parent)
}

/// Write `bytes` to `out` through a new private temporary file beside it,
/// then rename it into place. A symlink at `out` is replaced, never followed.
fn write_atomic(parent: &Path, out: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = parent.join(format!(".qa-evidence.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("create temporary evidence in {}", parent.display()))?;
    let written = (|| -> Result<()> {
        #[cfg(unix)]
        {
            // The umask may have narrowed or (umask 000) left the mode alone;
            // set it explicitly.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .context("set evidence mode 0600")?;
        }
        file.write_all(bytes).context("write evidence")?;
        file.sync_all().context("sync evidence")?;
        std::fs::rename(&tmp, out)
            .with_context(|| format!("rename evidence into {}", out.display()))?;
        Ok(())
    })();
    if let Err(e) = written {
        if let Err(cleanup) = std::fs::remove_file(&tmp)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.context(format!("and remove {}: {cleanup}", tmp.display())));
        }
        return Err(e);
    }
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("sync output directory {}", parent.display()))?;
    Ok(())
}

/// The outcome of one `qa-record` call.
pub enum Recorded {
    Written(Vec<u8>),
    Refused(Refusal),
}

/// Check the judgement on `stdin`, add the measured facts and write the
/// evidence. Refusals are checked before any file is read, so a refused
/// judgement never depends on the environment.
pub fn record(
    stdin: &[u8],
    inputs: &Path,
    repo: &Path,
    out: &Path,
    round: Option<&str>,
) -> Result<Recorded> {
    let j = match check_judgement(stdin) {
        Ok(j) => j,
        Err(r) => return Ok(Recorded::Refused(r)),
    };
    if let Some(r) = round {
        ensure!(
            !r.is_empty() && r.len() <= 200 && !r.chars().any(char::is_control),
            "--round must be one line of at most 200 bytes"
        );
    }
    let inputs = read_inputs(inputs, round)?;
    let head = trusted::git(repo, &["rev-parse", "--verify", "HEAD^{commit}"])
        .context("read the repository HEAD")?;
    if !([40, 64].contains(&head.len())
        && head.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    {
        bail!("repository HEAD is not a full commit id");
    }
    let parent = check_parent(out)?;
    let d = &inputs.declared_qa;
    let evidence = Evidence {
        qa_status: j.qa_status,
        qa_reason: derived_reason(j.qa_status),
        qa_judgement: j.qa_judgement,
        qa_judgement_reason: &j.qa_judgement_reason,
        qa_cited_files: &j.qa_cited_files,
        qa_scenarios: &j.qa_scenarios,
        qa_iterations: &j.qa_iterations,
        qa_handoff: &j.qa_handoff,
        qa_declared_command: DeclaredCommand {
            source: &d.source,
            dir: &d.dir,
            command: &d.command,
            exit_status: d.exit_status,
        },
        qa_round: round.unwrap_or(&inputs.qa_round),
        head_sha: &head,
        docs_only: &inputs.docs_only,
        qa_repo_type: &inputs.repo_type,
        qa_command: &d.command,
        qa_command_source: &d.source,
        qa_dir: &d.dir,
        qa_exit_code: d.exit_status.map(|c| c.to_string()).unwrap_or_default(),
        declared_qa: d,
        git_env_cleared: &inputs.git_env_cleared,
        run_env_cleared: &inputs.run_env_cleared,
    };
    let mut bytes = serde_json::to_vec(&evidence)?;
    bytes.push(b'\n');
    write_atomic(&parent, out, &bytes)?;
    Ok(Recorded::Written(bytes))
}

#[cfg(test)]
#[path = "qa_record_tests.rs"]
mod tests;
