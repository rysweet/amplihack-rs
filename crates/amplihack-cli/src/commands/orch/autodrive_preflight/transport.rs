//! Freeze publication policy before network access; repository rewrites never
//! reach execution. Only HTTPS to the exact PR source repository is supported.
use super::{git, output};
use anyhow::{Result, bail};
use std::{path::Path, process::Command};

pub(super) fn configure(
    command: &mut Command,
    at: &Path,
    url: &str,
    expected: Option<(&str, &str)>,
) -> Result<tempfile::TempDir> {
    if !url.starts_with("https://github.com/") || !url.ends_with(".git") {
        bail!("destination-policy: unsupported publication transport");
    }
    let dir = tempfile::tempdir()?;
    let format = git(at, &["rev-parse", "--show-object-format"])?;
    let init = format!("--object-format={format}");
    output(
        at,
        "git",
        &[
            "init",
            "--bare",
            "--template=",
            &init,
            dir.path()
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid temporary path"))?,
        ],
    )?;
    let baseline = std::fs::read(dir.path().join("config"))?;
    let config = output(at, "git", &["config", "--null", "--list"])?;
    let config_path = dir.path().join("config");
    let config_path = config_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid config path"))?;
    // Git intentionally does not consider a command-line-only remote to be
    // configured. Resolve rewrites through a real, private synthetic remote.
    for entry in config.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = std::str::from_utf8(entry)?;
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if key.starts_with("url.") || key.starts_with("remote.amplihack-preflight.") {
            output(
                at,
                "git",
                &["config", "--file", config_path, "--add", key, value],
            )?;
        }
    }
    output(
        at,
        "git",
        &[
            "config",
            "--file",
            config_path,
            "--add",
            "remote.amplihack-preflight.url",
            url,
        ],
    )?;
    let git_dir = dir
        .path()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid Git directory"))?;
    for push in [false, true] {
        let mut args = vec!["--git-dir", git_dir, "remote", "get-url", "--all"];
        if push {
            args.push("--push");
        }
        args.push("amplihack-preflight");
        let destinations = git(at, &args)?;
        if destinations.lines().collect::<Vec<_>>() != [url] {
            bail!(
                "destination-policy: effective fetch/push destination differs from the PR source repository"
            );
        }
    }
    // Neither the inspected rewrites nor any remote survives into execution.
    std::fs::write(dir.path().join("config"), baseline)?;
    let objects = at
        .join(git(at, &["rev-parse", "--git-path", "objects"])?)
        .canonicalize()?;
    std::fs::remove_dir_all(dir.path().join("objects"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&objects, dir.path().join("objects"))?;
    #[cfg(not(unix))]
    {
        bail!("destination-policy: isolated object access requires Unix");
    }
    // Hooks must see the original branch, index and Git configuration, not
    // the isolated network repository. Keep their ordinary cwd, arguments and
    // stdin (the pre-push ref list), and propagate their actual exit status.
    let original_hooks = at.join(git(at, &["rev-parse", "--git-path", "hooks"])?);
    let hooks = dir.path().join("policy-hooks");
    std::fs::create_dir(&hooks)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let wrapper = hooks.join("pre-push");
        let mut guard = String::from("#!/bin/sh\n");
        if let Some((reference, oid)) = expected {
            guard.push_str(&format!(
                "input=$(mktemp {}/push-input.XXXXXX) || exit 1\ntrap 'rm -f -- \"$input\"' EXIT\ncat > \"$input\" || exit 1\nmatched=0\nwhile read -r local_ref local_oid remote_ref remote_oid; do\n  [ \"$remote_ref\" = {} ] && [ \"$remote_oid\" = {} ] || {{ echo 'source branch changed during push negotiation' >&2; exit 1; }}\n  matched=1\ndone < \"$input\"\n[ \"$matched\" = 1 ] || {{ echo 'source branch changed during push negotiation' >&2; exit 1; }}\nexec < \"$input\"\nrm -f -- \"$input\"\ntrap - EXIT\n",
                quote(dir.path().to_str().unwrap()), quote(reference), quote(oid)
            ));
        }
        let hook = original_hooks.join("pre-push");
        if std::fs::metadata(&hook)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            let original_dir = git(at, &["rev-parse", "--absolute-git-dir"])?;
            let mut restore_config = String::new();
            for key in [
                "GIT_CONFIG_GLOBAL",
                "GIT_CONFIG_NOSYSTEM",
                "GIT_CONFIG_PARAMETERS",
                "GIT_CONFIG_COUNT",
            ] {
                match std::env::var(key) {
                    Ok(value) => {
                        restore_config.push_str(&format!("export {key}={}\n", quote(&value)))
                    }
                    Err(std::env::VarError::NotPresent) => {
                        restore_config.push_str(&format!("unset {key}\n"))
                    }
                    Err(error) => return Err(error.into()),
                }
            }

            let cwd = quote(
                at.to_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid hook cwd"))?,
            );
            let hook = quote(
                hook.to_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid hook path"))?,
            );
            let script = format!(
                "{guard}{restore_config}export GIT_DIR={} GIT_WORK_TREE={}\ncd {} || exit 1\nexec {} \"$@\"\n",
                quote(&original_dir),
                cwd,
                cwd,
                hook
            );
            std::fs::write(&wrapper, script)?;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
        } else {
            std::fs::write(&wrapper, guard)?;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("config"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    let mut frozen = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join("config"))?;
    // Snapshot authentication settings, but never URL rewrites, remotes, or
    // includes. Execution does not reread mutable repository/global config.
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT");
    command.arg(format!("--git-dir={}", dir.path().display()));
    command.arg(format!("--work-tree={}", at.display()));
    for entry in config.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = std::str::from_utf8(entry)?;
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if (key.starts_with("credential.") || key.starts_with("http."))
            && !key.to_ascii_lowercase().ends_with(".followredirects")
        {
            // Write through a file descriptor: neither config values nor inline
            // credential helpers may be exposed in subprocess arguments.
            use std::io::Write;
            let (section, rest) = key
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("invalid config key"))?;
            let escape = |s: &str| {
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\t', "\\t")
            };
            if let Some((subsection, name)) = rest.rsplit_once('.') {
                writeln!(
                    frozen,
                    "[{section} \"{}\"]\n{name} = \"{}\"",
                    escape(subsection),
                    escape(value)
                )?;
            } else {
                writeln!(frozen, "[{section}]\n{rest} = \"{}\"", escape(value))?;
            }
        }
    }
    command.args([
        "-c",
        &format!("core.hooksPath={}", hooks.display()),
        "-c",
        "http.followRedirects=false",
    ]);
    Ok(dir)
}
