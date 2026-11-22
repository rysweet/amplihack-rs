//! Issue #2957: process I/O for the user's own `gh`, with no policy.
//!
//! amplihack never reads gh's token. It runs `gh` itself, with the full
//! inherited environment, so gh honours `GH_TOKEN`, `GH_CONFIG_DIR` and
//! `GH_HOST` exactly as it does from a shell. The probe's output is discarded;
//! only its exit status is read.
//!
//! Every gh call runs under a wall-clock timeout, so a stalled gh cannot hold
//! up `amplihack update` or the launch-time update check. A timed-out probe
//! counts as "gh is not usable", like any other probe failure; a timed-out
//! `gh api` or `gh release download` is an error, with no anonymous fallback.

use super::GITHUB_REPO;
use super::network::sanitise_remote_text;
use crate::util::{run_output_with_timeout_limited, run_with_timeout_described};
use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Wall-clock limit for `gh auth status` and `gh api`. gh may make one
/// network round trip for each.
const GH_QUERY_TIMEOUT: Duration = Duration::from_secs(20);
/// Wall-clock limit for `gh release download`, sized for an archive at the
/// 512 MiB limit over a slow link.
const GH_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Most `gh api` stdout kept. The release JSON is far smaller; anything larger
/// is refused rather than parsed.
const MAX_GH_API_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Most stderr kept from any gh call. Only its first line is ever shown.
const MAX_GH_STDERR_BYTES: usize = 64 * 1024;

/// How to run gh. Deliberately has no `Debug` or `Display`, so a value can
/// never be formatted into a message or log.
pub(crate) struct GhCommand {
    program: OsString,
    /// Extra environment entries for the child. Empty in production; tests use
    /// it to point the child at private 0700 roots without changing the test
    /// process environment.
    env: Vec<(String, String)>,
    query_timeout: Duration,
    download_timeout: Duration,
}

impl Default for GhCommand {
    fn default() -> Self {
        Self {
            program: OsString::from("gh"),
            env: Vec::new(),
            query_timeout: GH_QUERY_TIMEOUT,
            download_timeout: GH_DOWNLOAD_TIMEOUT,
        }
    }
}

/// The parsed output of `gh api -i`. No `Debug`: the body is remote data.
pub(crate) struct GhApiResponse {
    pub(crate) status: u16,
    pub(crate) reset: Option<i64>,
    pub(crate) body: Vec<u8>,
}

impl GhCommand {
    #[cfg(test)]
    pub(crate) fn stand_in(program: impl Into<OsString>, env: Vec<(String, String)>) -> Self {
        Self {
            program: program.into(),
            env,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeouts(mut self, query: Duration, download: Duration) -> Self {
        self.query_timeout = query;
        self.download_timeout = download;
        self
    }

    /// Every gh invocation is built here: argv only, no shell, the environment
    /// inherited (never cleared), stdin closed.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command.args(args).stdin(Stdio::null());
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }

    /// `true` only when `gh auth status` starts and exits 0 within the query
    /// timeout. Its output is discarded, so nothing it prints can reach
    /// amplihack.
    pub(crate) fn usable(&self) -> bool {
        let mut command = self.command(&["auth", "status"]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let usable = match run_with_timeout_described(command, self.query_timeout, "gh auth status")
        {
            Ok(status) => status.success(),
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "gh auth status did not finish");
                false
            }
        };
        if !usable {
            tracing::debug!("gh is not usable; using the anonymous request");
        }
        usable
    }

    /// `gh api -i repos/<repo>/releases/latest`, parsed into status, reset
    /// header and body.
    pub(crate) fn latest_release_json(&self) -> Result<GhApiResponse> {
        let endpoint = format!("repos/{GITHUB_REPO}/releases/latest");
        // One byte over the limit is kept, so an oversized answer is detected.
        let output = run_output_with_timeout_limited(
            self.command(&["api", "-i", &endpoint]),
            self.query_timeout,
            MAX_GH_API_OUTPUT_BYTES + 1,
        )
        .context("failed to run gh api")?;
        if output.stdout.len() > MAX_GH_API_OUTPUT_BYTES {
            bail!("gh api output exceeded the limit of {MAX_GH_API_OUTPUT_BYTES} bytes");
        }
        parse_api_output(&output.stdout, &output.stderr, output.status.code())
    }

    /// `gh release download <tag> --repo <repo> --pattern ... --dir <dir>`.
    /// Runs once; the caller checks what it produced.
    pub(crate) fn download_release_assets(
        &self,
        tag: &str,
        patterns: &[&str],
        dir: &Path,
    ) -> Result<()> {
        let dir_text = dir
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("the download directory path is not valid UTF-8"))?;
        let mut args = vec!["release", "download", tag, "--repo", GITHUB_REPO];
        for pattern in patterns {
            args.push("--pattern");
            args.push(pattern);
        }
        args.push("--dir");
        args.push(dir_text);
        let output = run_output_with_timeout_limited(
            self.command(&args),
            self.download_timeout,
            MAX_GH_STDERR_BYTES,
        )
        .context("failed to run gh release download")?;
        if !output.status.success() {
            bail!(
                "gh release download {tag} failed ({}): {}",
                describe_exit(output.status.code()),
                stderr_text(&output.stderr)
            );
        }
        Ok(())
    }
}

fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit status {code}"),
        None => "terminated by a signal".to_string(),
    }
}

fn stderr_text(stderr: &[u8]) -> String {
    let text = sanitise_remote_text(&String::from_utf8_lossy(stderr));
    if text.is_empty() {
        "gh gave no error text".to_string()
    } else {
        text
    }
}

/// Parses `gh api -i` output: a status line (`HTTP/<ver> <code> ...`), header
/// lines up to the first empty line, then the body. A non-zero exit with a
/// status line is still an answer from GitHub and is returned as `Ok`.
pub(crate) fn parse_api_output(
    stdout: &[u8],
    stderr: &[u8],
    exit_code: Option<i32>,
) -> Result<GhApiResponse> {
    let fail = || {
        anyhow::anyhow!(
            "gh api failed ({}): {}",
            describe_exit(exit_code),
            stderr_text(stderr)
        )
    };

    let mut rest = stdout;
    let status_line = next_line(&mut rest).ok_or_else(fail)?;
    let status_line = std::str::from_utf8(status_line).map_err(|_| fail())?;
    let mut tokens = status_line.split_ascii_whitespace();
    let status = match (tokens.next(), tokens.next()) {
        (Some(version), Some(code)) if version.starts_with("HTTP/") => {
            code.parse::<u16>().map_err(|_| fail())?
        }
        _ => return Err(fail()),
    };

    let mut reset = None;
    while let Some(line) = next_line(&mut rest) {
        if line.is_empty() {
            break;
        }
        let line = String::from_utf8_lossy(line);
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("x-ratelimit-reset")
        {
            reset = value.trim().parse::<i64>().ok();
        }
    }

    Ok(GhApiResponse {
        status,
        reset,
        body: rest.to_vec(),
    })
}

/// Takes the next line (without its `\n` or `\r\n`) off the front of `rest`.
fn next_line<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    if rest.is_empty() {
        return None;
    }
    let (line, tail) = match rest.iter().position(|&b| b == b'\n') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (*rest, &rest[rest.len()..]),
    };
    *rest = tail;
    Some(line.strip_suffix(b"\r").unwrap_or(line))
}
