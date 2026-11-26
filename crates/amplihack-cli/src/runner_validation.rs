//! Read-only capability and Cargo receipt validation shared by delivery paths.
use anyhow::{Result, anyhow};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, path::Path};
fn error(code: &str) -> anyhow::Error {
    anyhow!("{code}")
}

/// The most characters a probe cause may hold.
const CAUSE_LIMIT: usize = 512;

/// `RUNNER_PROBE_FAILED` with the reason beside it (issue #2931).
///
/// The display text is exactly the code and there is no source, so every
/// caller that compares or prints the error sees what it saw before. The
/// cause is read with [`probe_cause`]. It is built from step names written in
/// this source, an exit status, and an operating-system error (number and
/// standard text): never a path, an argument, an environment value or a byte
/// the runner wrote.
pub(crate) struct ProbeFailure {
    cause: String,
}

impl ProbeFailure {
    /// The one constructor: the cause is cut to [`CAUSE_LIMIT`] characters.
    pub(crate) fn new(cause: impl Into<String>) -> Self {
        let cause: String = cause.into();
        let cause = match cause.char_indices().nth(CAUSE_LIMIT) {
            Some((end, _)) => cause[..end].to_string(),
            None => cause,
        };
        Self { cause }
    }
}

impl fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RUNNER_PROBE_FAILED")
    }
}

// Written by hand so that the debug form obeys the same rule as the cause:
// control characters escaped, nothing beyond the capped text.
impl fmt::Debug for ProbeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RUNNER_PROBE_FAILED ({})", printable(&self.cause))
    }
}

impl std::error::Error for ProbeFailure {}

fn probe_failed(cause: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ProbeFailure::new(cause))
}

/// `text` with every control character written as an escape, so that a cause
/// cannot move the cursor or start a new line on a terminal or in a log.
fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// The cause of a probe failure anywhere in `error`'s chain, however many
/// context layers wrap it. `None` for any other error.
pub(crate) fn probe_cause(error: &anyhow::Error) -> Option<String> {
    error
        .chain()
        .find_map(|e| e.downcast_ref::<ProbeFailure>())
        .map(|failure| failure.cause.clone())
}

/// What a human-readable line prints after the code: `: <cause>` with control
/// characters escaped, or nothing when `error` is not a probe failure.
pub(crate) fn cause_suffix(error: &anyhow::Error) -> String {
    probe_cause(error)
        .map(|cause| format!(": {}", printable(&cause)))
        .unwrap_or_default()
}

/// `error` under `context`, as `.context(..)` would give, with the probe's
/// cause added to the sentence when there is one: `<context>
/// (RUNNER_PROBE_FAILED: <cause>)`. A caller that prints only the outermost
/// message then still shows why the probe failed.
pub(crate) fn context_with_cause(error: anyhow::Error, context: &str) -> anyhow::Error {
    let suffix = cause_suffix(&error);
    if suffix.is_empty() {
        error.context(context.to_string())
    } else {
        error.context(format!("{context} (RUNNER_PROBE_FAILED{suffix})"))
    }
}

/// An I/O error as its operating-system number and standard text. An error
/// with no number gives its kind, never a message that could hold a path.
fn os_error_text(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => std::io::Error::from_raw_os_error(code).to_string(),
        None => format!("{:?}", error.kind()),
    }
}

/// How the runner ended, in words this source wrote.
fn exit_cause(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("runner exited with status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("runner ended by signal {signal}");
        }
    }
    "runner ended without an exit status".to_string()
}

// Reject ambiguous top-level properties rather than accepting last-value wins.
struct Report(BTreeMap<String, Value>);
impl<'de> Deserialize<'de> for Report {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Report;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique report properties")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Report, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate report property"));
                    }
                }
                Ok(Report(fields))
            }
        }
        d.deserialize_map(Fields)
    }
}

pub(crate) fn probe(binary: &Path, provider: &str) -> Result<Value> {
    // The capture chain is static step text and operating-system errors
    // (`runner_probe.rs`), so it may be kept as the cause.
    let output =
        crate::runner_probe::capture(binary).map_err(|e| probe_failed(format!("{e:#}")))?;
    if !output.status.success() {
        return Err(probe_failed(exit_cause(output.status)));
    }
    let Report(fields) =
        serde_json::from_slice(&output.stdout).map_err(|_| error("RUNNER_REPORT_INVALID"))?;
    let report = serde_json::to_value(fields)?;
    if report["schema_version"].as_u64() != Some(1)
        || !report["version"]
            .as_str()
            .is_some_and(|v| !v.trim().is_empty())
        || !report["capabilities"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(error("RUNNER_REPORT_INVALID"));
    }
    if !report["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "durable_recovery_v1")
    {
        return Err(error(
            "RUNNER_RECOVERY_CAPABILITY_MISSING: durable_recovery_v1",
        ));
    }
    if provider == "codex"
        && !report["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "codex_exec")
    {
        return Err(error("RUNNER_CAPABILITY_MISSING"));
    }
    Ok(report)
}

/// Revisions of every receipt entry that installed the `recipe-runner-rs`
/// package and binary from exactly `repository` (a `?query` is allowed).
fn receipt_entries<'a>(receipt: &'a Value, repository: &str) -> Vec<&'a str> {
    let Some(entries) = receipt["installs"].as_object() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(source, record)| {
            let rest = source.strip_prefix("recipe-runner-rs ")?;
            let (version, origin) = rest.split_once(" (git+")?;
            let origin = origin.strip_suffix(')')?;
            let (url, sha) = origin.rsplit_once('#')?;
            let lists_binary = record["bins"]
                .as_array()
                .is_some_and(|bins| bins.iter().any(|b| b == "recipe-runner-rs"));
            (!version.is_empty() && url.split('?').next() == Some(repository) && lists_binary)
                .then_some(sha)
        })
        .collect()
}

/// Receipts larger than this are not parsed.
const RECEIPT_LIMIT: u64 = 1024 * 1024;

/// The revision Cargo recorded for the `recipe-runner-rs` installed from
/// `repository` under `cargo_home`. `None` unless exactly one receipt entry
/// matches and its revision is a full lowercase SHA; an unreadable,
/// oversize or malformed receipt is also `None`.
pub(crate) fn installed_revision(cargo_home: &Path, repository: &str) -> Option<String> {
    use std::io::Read;
    let unknown = |reason: &str| {
        tracing::debug!("recipe-runner-rs receipt gives no installed revision: {reason}");
        None
    };
    let file = match std::fs::File::open(cargo_home.join(".crates2.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return unknown(&format!("cannot open receipt ({:?})", e.kind())),
    };
    let mut bytes = Vec::new();
    if let Err(e) = file.take(RECEIPT_LIMIT + 1).read_to_end(&mut bytes) {
        return unknown(&format!("cannot read receipt ({:?})", e.kind()));
    }
    if bytes.len() as u64 > RECEIPT_LIMIT {
        return unknown("receipt is larger than 1 MiB");
    }
    let Ok(receipt) = serde_json::from_slice::<Value>(&bytes) else {
        return unknown("receipt is not valid JSON");
    };
    let mut revisions = receipt_entries(&receipt, repository);
    revisions.sort_unstable();
    revisions.dedup();
    match revisions.as_slice() {
        [sha] if crate::runner_ancestry::is_sha(sha) => Some((*sha).to_string()),
        [] => unknown("no entry for the runner repository"),
        [_] => unknown("the recorded revision is not a full SHA"),
        many => unknown(&format!(
            "{} entries match the runner repository",
            many.len()
        )),
    }
}

pub(crate) fn provenance(
    binary: &Path,
    cargo_home: &Path,
    repository: &str,
    revision: &str,
) -> Result<()> {
    let selected = binary
        .canonicalize()
        .map_err(|_| error("RUNNER_BINARY_SHADOWED"))?;
    let managed = cargo_home
        .join("bin/recipe-runner-rs")
        .canonicalize()
        .map_err(|_| error("RUNNER_BINARY_SHADOWED"))?;
    if selected != managed {
        return Err(error("RUNNER_BINARY_SHADOWED"));
    }
    let bytes = std::fs::read(cargo_home.join(".crates2.json"))
        .map_err(|_| error("RUNNER_RECEIPT_MISSING"))?;
    let receipt: Value =
        serde_json::from_slice(&bytes).map_err(|_| error("RUNNER_PROVENANCE_MISMATCH"))?;
    let valid = receipt_entries(&receipt, repository).contains(&revision);
    if !valid {
        return Err(error("RUNNER_PROVENANCE_MISMATCH"));
    }
    Ok(())
}

#[derive(clap::Subcommand, Debug)]
pub enum InternalCommands {
    ValidateRecipeRunner(ValidationArgs),
}
#[derive(clap::Args, Debug)]
pub struct ValidationArgs {
    #[arg(long, value_parser = ["claude", "copilot", "codex"])]
    provider: String,
    /// Additional execution contracts required by the delivery caller.
    #[arg(long = "require-capability")]
    required_capabilities: Vec<String>,
    #[arg(long)]
    binary: std::path::PathBuf,
    #[arg(long)]
    cargo_home: Option<std::path::PathBuf>,
    #[arg(long)]
    repository: Option<String>,
    #[arg(long)]
    revision: Option<String>,
}

pub(crate) fn run(command: InternalCommands) -> Result<()> {
    let InternalCommands::ValidateRecipeRunner(args) = command;
    let result = (|| -> Result<Value> {
        if !args.binary.is_absolute() {
            return Err(error("INVALID_VALIDATION_REQUEST"));
        }
        let managed = match (&args.cargo_home, &args.repository, &args.revision) {
            (None, None, None) => false,
            (Some(home), Some(repo), Some(rev))
                if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                provenance(&args.binary, home, repo, rev)?;
                true
            }
            _ => return Err(error("INVALID_VALIDATION_REQUEST")),
        };
        let runner = probe(&args.binary, &args.provider)?;
        for capability in &args.required_capabilities {
            if capability.len() > 64
                || !capability
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(error("INVALID_VALIDATION_REQUEST"));
            }
            if !runner["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == capability)
            {
                return Err(anyhow!("RUNNER_CAPABILITY_MISSING {capability}"));
            }
        }
        let provenance = if managed {
            json!({"kind":"managed", "package":"recipe-runner-rs", "repository":args.repository,
                "revision":args.revision, "cargo_home":args.cargo_home})
        } else {
            json!({"kind":"explicit"})
        };
        Ok(
            json!({"schema_version":1,"provider":args.provider,"runner":runner,
            "binary":args.binary.canonicalize().map_err(|e| probe_failed(format!("canonicalize binary: {}", os_error_text(&e))))?, "provenance":provenance}),
        )
    })();
    match result {
        Ok(value) => {
            println!("{value}");
            Ok(())
        }
        Err(error) => {
            let code = error.to_string();
            let message = match code.as_str() {
                "RUNNER_RECOVERY_CAPABILITY_MISSING: durable_recovery_v1" => {
                    "runner must advertise durable_recovery_v1; install the supported pinned runner"
                }
                "RUNNER_CAPABILITY_MISSING" => {
                    "runner must advertise codex_exec; fix or unset the override"
                }
                "RUNNER_REPORT_INVALID" => {
                    "runner must emit schema 1, a nonblank version and string capabilities"
                }
                "RUNNER_PROBE_FAILED" => {
                    "runner capability probe failed; check executable compatibility and permissions"
                }
                "RUNNER_RECEIPT_MISSING" => "Cargo receipt missing; reinstall the managed runner",
                "RUNNER_BINARY_SHADOWED" => {
                    "selected runner differs from the managed Cargo binary; fix PATH"
                }
                "RUNNER_PROVENANCE_MISMATCH" => {
                    "Cargo receipt does not bind package, repository, revision and binary; reinstall"
                }
                _ => {
                    "provide an absolute binary and all or none of cargo-home, repository and revision"
                }
            };
            let mut details = json!({"provider":args.provider});
            if let Some(cause) = probe_cause(&error) {
                details["cause"] = Value::String(printable(&cause));
            }
            eprintln!(
                "{}",
                json!({"error":{"code":code,"message":message,"details":details}})
            );
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod installed_revision_tests {
    use super::installed_revision;
    use std::path::Path;

    const URL: &str = "https://github.com/rysweet/amplihack-recipe-runner";
    const SHA: &str = "4d5187eaf95836a549e6e68725f8f853d0dc7034";
    const OTHER: &str = "b4a82e7000000000000000000000000000000000";

    fn write(home: &Path, body: &str) {
        std::fs::write(home.join(".crates2.json"), body).unwrap();
    }

    fn entry(source: &str, bins: &str) -> String {
        format!(r#"{:?}:{{"bins":{bins}}}"#, source)
    }

    fn receipt(entries: &[String]) -> String {
        format!(r#"{{"installs":{{{}}}}}"#, entries.join(","))
    }

    fn runner(url: &str, sha: &str) -> String {
        entry(
            &format!("recipe-runner-rs 0.5.0 (git+{url}#{sha})"),
            r#"["recipe-runner-rs"]"#,
        )
    }

    #[test]
    fn exact_key_yields_the_revision() {
        let home = tempfile::tempdir().unwrap();
        write(home.path(), &receipt(&[runner(URL, SHA)]));
        assert_eq!(installed_revision(home.path(), URL).as_deref(), Some(SHA));
    }

    #[test]
    fn url_with_query_yields_the_revision() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            &receipt(&[runner(&format!("{URL}?rev={SHA}"), SHA)]),
        );
        assert_eq!(installed_revision(home.path(), URL).as_deref(), Some(SHA));
    }

    #[test]
    fn other_repositories_and_lookalikes_are_ignored() {
        for url in [
            "https://github.com/someone/amplihack-recipe-runner",
            "https://github.com/rysweet/amplihack-recipe-runner-evil",
            "https://github.com/rysweet/amplihack-recipe-runner/evil",
            "https://github.com/rysweet/amplihack-recipe-runner.git",
        ] {
            let home = tempfile::tempdir().unwrap();
            write(home.path(), &receipt(&[runner(url, SHA)]));
            assert_eq!(installed_revision(home.path(), URL), None, "{url}");
        }
    }

    #[test]
    fn invalid_revisions_are_rejected() {
        let upper = SHA.to_uppercase();
        let non_hex = format!("{}g", &SHA[..39]);
        let too_long = format!("{SHA}0");
        for sha in [&SHA[..39], &upper, &non_hex, "", &too_long] {
            let home = tempfile::tempdir().unwrap();
            write(home.path(), &receipt(&[runner(URL, sha)]));
            assert_eq!(installed_revision(home.path(), URL), None, "{sha:?}");
        }
    }

    #[test]
    fn two_distinct_runner_entries_are_ambiguous() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            &receipt(&[runner(URL, SHA), {
                entry(
                    &format!("recipe-runner-rs 0.4.0 (git+{URL}#{OTHER})"),
                    r#"["recipe-runner-rs"]"#,
                )
            }]),
        );
        assert_eq!(installed_revision(home.path(), URL), None);
    }

    #[test]
    fn unrelated_packages_do_not_count_as_a_second_entry() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            &receipt(&[
                runner(URL, SHA),
                entry(
                    &format!("other-tool 1.0.0 (git+https://example.invalid/x#{OTHER})"),
                    r#"["other-tool"]"#,
                ),
            ]),
        );
        assert_eq!(installed_revision(home.path(), URL).as_deref(), Some(SHA));
    }

    #[test]
    fn an_entry_without_the_runner_binary_is_ignored() {
        for bins in [r#"[]"#, r#"["something-else"]"#, r#""recipe-runner-rs""#] {
            let home = tempfile::tempdir().unwrap();
            write(
                home.path(),
                &receipt(&[entry(
                    &format!("recipe-runner-rs 0.5.0 (git+{URL}#{SHA})"),
                    bins,
                )]),
            );
            assert_eq!(installed_revision(home.path(), URL), None, "{bins}");
        }
    }

    #[test]
    fn missing_malformed_and_oversize_receipts_yield_none() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(installed_revision(home.path(), URL), None, "no receipt");
        for body in ["", "{", "[]", r#"{"installs":[]}"#, "\u{0}\u{1}"] {
            write(home.path(), body);
            assert_eq!(installed_revision(home.path(), URL), None, "{body:?}");
        }
        let mut big = receipt(&[runner(URL, SHA)]);
        big.push_str(&" ".repeat(1024 * 1024 + 1));
        write(home.path(), &big);
        assert_eq!(installed_revision(home.path(), URL), None, "over 1 MiB");
    }

    #[test]
    fn provenance_still_accepts_the_same_receipt() {
        // receipt_entries is shared with provenance(); the refactor must not
        // change what provenance accepts.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("bin")).unwrap();
        let binary = home.path().join("bin/recipe-runner-rs");
        std::fs::write(&binary, "").unwrap();
        write(home.path(), &receipt(&[runner(URL, SHA)]));
        super::provenance(&binary, home.path(), URL, SHA).unwrap();
        assert!(super::provenance(&binary, home.path(), URL, OTHER).is_err());
        write(
            home.path(),
            &receipt(&[runner(
                "https://github.com/rysweet/amplihack-recipe-runner-evil",
                SHA,
            )]),
        );
        assert!(super::provenance(&binary, home.path(), URL, SHA).is_err());
    }
}

/// Issue #2931 (PROBE): the probe failure keeps its code as its whole display
/// text and carries a cause beside it.
///
/// Contract: `ProbeFailure::new(cause)` holds at most 512 characters;
/// `Display` is exactly `RUNNER_PROBE_FAILED` and `source()` is `None`, so the
/// code that callers compare is unchanged; `probe_cause` finds the cause in an
/// `anyhow` chain however many context layers wrap it; `cause_suffix` is the
/// text printed after the code on a human-readable line, empty when the error
/// is not a probe failure.
#[cfg(test)]
mod probe_cause_tests {
    use super::{ProbeFailure, cause_suffix, probe_cause};
    use anyhow::anyhow;
    use std::error::Error as _;

    #[test]
    fn the_display_text_is_exactly_the_code_and_there_is_no_source() {
        let failure = ProbeFailure::new("runner exited with status 17");
        assert_eq!(failure.to_string(), "RUNNER_PROBE_FAILED");
        assert!(failure.source().is_none());
    }

    #[test]
    fn the_cause_is_found_through_two_layers_of_context() {
        let error = anyhow::Error::new(ProbeFailure::new("runner exited with status 17"))
            .context("first layer")
            .context("second layer");
        assert_eq!(
            probe_cause(&error).as_deref(),
            Some("runner exited with status 17")
        );
    }

    #[test]
    fn an_unrelated_error_has_no_cause_and_no_suffix() {
        let error = anyhow!("RUNNER_REPORT_INVALID");
        assert_eq!(probe_cause(&error), None);
        assert_eq!(cause_suffix(&error), "");
    }

    #[test]
    fn the_suffix_follows_the_code_after_a_colon() {
        let error = anyhow::Error::new(ProbeFailure::new("runner exited with status 17"));
        assert_eq!(cause_suffix(&error), ": runner exited with status 17");
    }

    #[test]
    fn a_cause_is_capped_at_512_characters() {
        let long = "x".repeat(600);
        let error = anyhow::Error::new(ProbeFailure::new(long));
        let cause = probe_cause(&error).expect("a cause");
        assert_eq!(cause.chars().count(), 512);
    }

    #[test]
    fn control_characters_are_escaped_in_the_human_readable_suffix() {
        let error = anyhow::Error::new(ProbeFailure::new("line one\nline two\u{1b}[31m\ttab"));
        let suffix = cause_suffix(&error);
        assert!(
            suffix.chars().all(|c| !c.is_control()),
            "no raw control character in {suffix:?}"
        );
        assert!(suffix.contains("line one\\nline two"), "{suffix:?}");
    }
}
