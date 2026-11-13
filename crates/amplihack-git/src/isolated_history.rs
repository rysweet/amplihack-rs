//! Complete immutable histories in a disposable repository, without local refs.
use crate::workspace_validation::run as source_run;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct History {
    pub directory: tempfile::TempDir,
    pub advertised: BTreeSet<String>,
    pub refs: BTreeMap<String, String>,
}
impl History {
    pub fn obtain(source: &Path, destination: &str, proposed: &[String]) -> Result<Self> {
        let source_path = source
            .canonicalize()
            .context("cannot resolve publication source")?;
        let source = source_path.as_path();
        let destination = resolve_destination(source, destination)?;
        let destination = destination.as_str();
        require_literal_destination(source, destination)?;
        // This is the sole advertisement for this destination and push attempt.
        let transport = transport_config(source)?;
        let advertisement = source_run(source, &["ls-remote", destination])
            .context("destination advertisement failed; publication stopped")?;
        let mut tips = BTreeSet::new();
        let mut refs = BTreeMap::new();
        for line in advertisement.lines() {
            let (oid, reference) = line
                .split_once('\t')
                .context("invalid destination advertisement")?;
            if !oid.bytes().all(|b| b.is_ascii_hexdigit())
                || ![40, 64].contains(&oid.len())
                || reference.is_empty()
            {
                bail!("invalid advertised object or ref");
            }
            refs.insert(reference.to_owned(), oid.to_owned());
            tips.insert(oid.to_owned());
        }
        let directory = tempfile::Builder::new()
            .prefix("amplihack-publication-")
            .tempdir()?;
        let cwd = directory.path();
        let template = tempfile::tempdir()?;
        let init = command(cwd)
            .args(["init", "--bare", "--quiet"])
            .arg(format!("--template={}", template.path().display()))
            .output()?;
        if !init.status.success() {
            bail!("cannot initialise isolated history");
        }
        std::fs::write(cwd.join("config"), "[core]\n\tbare = true\n")?;

        if !tips.is_empty() {
            let mut args = vec![
                "fetch",
                "--atomic",
                "--no-tags",
                "--no-write-fetch-head",
                destination,
            ];
            args.extend(tips.iter().map(String::as_str));
            let mut fetch = command(cwd);
            for (i, (key, value)) in transport.iter().enumerate() {
                fetch
                    .env(format!("GIT_CONFIG_KEY_{i}"), key)
                    .env(format!("GIT_CONFIG_VALUE_{i}"), value);
            }
            fetch.env("GIT_CONFIG_COUNT", transport.len().to_string());
            let output = fetch.args(&args).output()?;
            if !output.status.success() {
                bail!(
                    "destination history fetch failed; publication stopped: {}",
                    crate::publication_validation::redact(&String::from_utf8_lossy(&output.stderr))
                );
            }
        }
        let source_name = source.to_str().context("unsupported checkout filename")?;
        let mut args = vec![
            "fetch",
            "--atomic",
            "--no-tags",
            "--no-write-fetch-head",
            source_name,
        ];
        args.extend(proposed.iter().map(String::as_str));
        run(cwd, &args).context("proposed history transfer failed")?;
        // Refuse missing objects, shallow boundaries and incomplete promised history.
        if run(cwd, &["rev-parse", "--is-shallow-repository"])?.trim() != "false" {
            bail!("incomplete publication history");
        }
        let mut advertised = BTreeSet::new();
        for tip in tips {
            let out = command(cwd)
                .args(["rev-parse", "--verify", &format!("{tip}^{{commit}}")])
                .output()?;
            if out.status.success() {
                advertised.insert(String::from_utf8(out.stdout)?.trim().to_owned());
            } else {
                bytes(cwd, &["cat-file", "-e", &tip])?;
            }
        }
        let mut check = vec!["rev-list", "--objects", "--missing=error"];
        check.extend(proposed.iter().map(String::as_str));
        check.extend(advertised.iter().map(String::as_str));
        bytes(cwd, &check).context("incomplete destination or proposed history")?;
        Ok(Self {
            directory,
            advertised,
            refs,
        })
    }
}

/// History interpretation never inherits user templates or configuration.
pub(crate) fn command(root: &Path) -> std::process::Command {
    let mut command = crate::command_in(root);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    command
}
pub(crate) fn bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(root).args(args).output()?;
    if !output.status.success() {
        bail!(
            "isolated history command failed: {}",
            crate::publication_validation::redact(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(output.stdout)
}
pub(crate) fn run(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(bytes(root, args)?)?)
}

/// Copy only transport settings. History interpretation uses no user config.
fn transport_config(source: &Path) -> Result<Vec<(String, String)>> {
    let output = crate::command_in(source)
        .args([
            "config",
            "--null",
            "--get-regexp",
            "^(credential\\.|http\\.|https\\.|core\\.sshCommand$|core\\.gitProxy$)",
        ])
        .output()?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        bail!("cannot read destination transport configuration");
    }
    let mut entries = Vec::new();
    for entry in output.stdout.split(|b| *b == 0).filter(|v| !v.is_empty()) {
        let entry = std::str::from_utf8(entry).context("unsupported credential configuration")?;
        let (key, value) = entry
            .split_once('\n')
            .context("invalid credential configuration")?;
        entries.push((key.to_owned(), value.to_owned()));
    }
    Ok(entries)
}

/// Git resolves local relative URLs against its working directory. Freeze that
/// meaning before acquiring history in a different disposable repository.
pub(crate) fn resolve_destination(source: &Path, destination: &str) -> Result<String> {
    if destination.starts_with('-') || destination.is_empty() {
        bail!("invalid push destination");
    }
    let local = destination
        .find(':')
        .is_none_or(|colon| destination[..colon].contains('/'));
    if local && !Path::new(destination).is_absolute() {
        return Ok(source
            .join(destination)
            .canonicalize()
            .context("cannot resolve local push destination")?
            .to_str()
            .context("unsupported destination filename")?
            .to_owned());
    }
    Ok(destination.to_owned())
}

fn destination_config(source: &Path) -> Result<Vec<(String, String)>> {
    let output = crate::command_in(source)
        .args([
            "config",
            "--null",
            "--get-regexp",
            "^(url\\.|remote\\..*\\.(url|pushurl)$)",
        ])
        .output()?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        bail!("cannot inspect destination URL configuration");
    }
    let mut entries = Vec::new();
    for entry in output
        .stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
    {
        let entry = std::str::from_utf8(entry).context("unsupported URL configuration encoding")?;
        let (key, value) = entry
            .split_once('\n')
            .context("invalid URL configuration record")?;
        if key.chars().any(char::is_control) || value.chars().any(char::is_control) {
            bail!("control character in destination URL configuration");
        }
        entries.push((key.to_owned(), value.to_owned()));
    }
    Ok(entries)
}
pub(crate) fn check_destination_config(source: &Path) -> Result<()> {
    destination_config(source)?;
    Ok(())
}

/// Expanded URLs must retain their meaning when passed to push and ls-remote.
/// Git applies pushInsteadOf again for an explicit URL. Refuse rewrite chains
/// rather than validate one destination then publish to a different one.
pub(crate) fn require_literal_destination(source: &Path, destination: &str) -> Result<()> {
    if destination.chars().any(char::is_control) {
        bail!("control character in push destination");
    }
    for (key, prefix) in destination_config(source)? {
        if (key.ends_with(".insteadof") || key.ends_with(".pushinsteadof"))
            && destination.starts_with(&prefix)
        {
            bail!("expanded push destination would be rewritten again; publication refused");
        }
    }
    Ok(())
}
