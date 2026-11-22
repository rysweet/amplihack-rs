//! Issue #2957: release lookup and download through gh's login.
//!
//! These tests are written before the implementation and define its contract:
//!
//! * `update::gh::GhCommand` (`stand_in(program, env)`, `usable()`,
//!   `latest_release_json()`, `download_release_assets()`), plus the pure parser
//!   `update::gh::parse_api_output(stdout, stderr, exit_code)`.
//! * `network::fetch_latest_release_with(gh, api_url, fetch)`,
//!   `network::refusal_message_at`, `network::sanitise_remote_text` and the
//!   `cfg(test)` fetcher `network::http_get_with_retry_unvalidated`.
//! * `checksum::verify_sha256_bytes(archive, checksum, label)`.
//! * `install::fetch_release_archive(gh, release) -> Result<Vec<u8>>`.
//!
//! A stand-in `gh` shell script and a loopback stand-in API are used. Every
//! environment change is set on the child through `GhCommand::stand_in`; the
//! test process environment is never changed, and the real gh configuration,
//! `~/.amplihack` and `~/.local/bin` are never touched.

use super::super::checksum::verify_sha256_bytes;
use super::super::gh::{self, GhCommand};
use super::super::install::fetch_release_archive;
use super::super::network::{
    fetch_latest_release_with, http_get_with_retry_unvalidated, refusal_message_at,
    sanitise_remote_text,
};
use super::super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SENTINEL: &str = "amplihack-sentinel-2957";
const RATE_LIMIT_TEXT: &str = "API rate limit exceeded for 203.0.113.9";
/// 2026-11-22 14:05:00 UTC.
const RESET_1405: i64 = 1_795_356_300;

// ---------------------------------------------------------------------------
// Pure unit tests
// ---------------------------------------------------------------------------

#[test]
fn refusal_message_formats() {
    let now = RESET_1405 - 23 * 60;
    let msg = refusal_message_at(403, Some(RATE_LIMIT_TEXT), Some(RESET_1405), now);
    assert!(
        msg.contains("GitHub refused the request (HTTP 403)"),
        "{msg}"
    );
    assert!(msg.contains(RATE_LIMIT_TEXT), "{msg}");
    assert!(msg.contains("resets at 14:05 UTC (in 23 minutes)"), "{msg}");
    assert!(msg.contains("AMPLIHACK_NO_UPDATE_CHECK"), "{msg}");
    assert!(!msg.contains("wait ~60 minutes"), "{msg}");
    assert!(!msg.contains("retried automatically"), "{msg}");

    // Already past, and under a minute away, both read "under a minute".
    for now in [RESET_1405 + 120, RESET_1405 - 30] {
        let msg = refusal_message_at(429, Some(RATE_LIMIT_TEXT), Some(RESET_1405), now);
        assert!(msg.contains("(in under a minute)"), "{msg}");
        assert!(msg.contains("HTTP 429"), "{msg}");
    }

    let msg = refusal_message_at(
        403,
        Some(RATE_LIMIT_TEXT),
        Some(RESET_1405),
        RESET_1405 - 60,
    );
    assert!(msg.contains("(in 1 minute)"), "{msg}");

    // Minutes are floored.
    let msg = refusal_message_at(
        403,
        Some(RATE_LIMIT_TEXT),
        Some(RESET_1405),
        RESET_1405 - 119,
    );
    assert!(msg.contains("(in 1 minute)"), "{msg}");

    let msg = refusal_message_at(403, Some(RATE_LIMIT_TEXT), None, RESET_1405);
    assert!(msg.contains("GitHub did not give a reset time."), "{msg}");
    assert!(!msg.contains("resets at"), "{msg}");

    // An out-of-range reset is treated as absent.
    let msg = refusal_message_at(403, Some(RATE_LIMIT_TEXT), Some(i64::MAX), RESET_1405);
    assert!(msg.contains("GitHub did not give a reset time."), "{msg}");
}

#[test]
fn refusal_message_supplies_rate_limit_phrase_only_when_missing() {
    let msg = refusal_message_at(403, Some("Resource not accessible"), None, 0);
    assert!(msg.contains("Resource not accessible"), "{msg}");
    assert!(msg.contains("rate limit"), "{msg}");

    let msg = refusal_message_at(403, None, None, 0);
    assert!(msg.contains("rate limit"), "{msg}");

    let msg = refusal_message_at(403, Some("You have exceeded a rate limit"), None, 0);
    assert_eq!(msg.matches("rate limit").count(), 1, "{msg}");
}

#[test]
fn sanitiser_takes_first_line_strips_controls_and_truncates() {
    assert_eq!(
        sanitise_remote_text("  first line.\nsecond line  "),
        "first line"
    );
    assert_eq!(sanitise_remote_text("a\u{1b}[31mred\u{7}b"), "a[31mredb");
    assert_eq!(sanitise_remote_text("tab\there"), "tabhere");
    assert_eq!(sanitise_remote_text(""), "");
    let long = "x".repeat(1000);
    assert_eq!(sanitise_remote_text(&long).chars().count(), 300);
}

#[test]
fn gh_api_output_parsing() {
    let ok = b"HTTP/2.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"tag_name\":\"v1.0.0\"}";
    let parsed = gh::parse_api_output(ok, b"", Some(0)).unwrap();
    assert_eq!(parsed.status, 200);
    assert_eq!(parsed.reset, None);
    assert_eq!(parsed.body, b"{\"tag_name\":\"v1.0.0\"}");

    // Mixed-case header, HTTP/1.1, LF-only endings, non-zero exit with a status line.
    let limited = b"HTTP/1.1 403 Forbidden\nX-RateLimit-Reset: 1795356300\nx-other: 1\n\n{\"message\":\"API rate limit exceeded\"}";
    let parsed = gh::parse_api_output(limited, b"gh: API rate limit exceeded (HTTP 403)", Some(1))
        .expect("a non-zero exit with a status line is still a parsed answer");
    assert_eq!(parsed.status, 403);
    assert_eq!(parsed.reset, Some(RESET_1405));
    assert!(String::from_utf8_lossy(&parsed.body).contains("API rate limit exceeded"));

    // No status line: stderr becomes the message.
    // `GhApiResponse` has no `Debug` on purpose, so `.err()` stands in for `unwrap_err`.
    let err = gh::parse_api_output(b"", b"  gh: network unreachable\n", Some(1))
        .err()
        .expect("no status line is an error");
    let text = format!("{err:#}");
    assert!(text.contains("gh api failed"), "{text}");
    assert!(text.contains("gh: network unreachable"), "{text}");

    let err = gh::parse_api_output(b"", b"", Some(4))
        .err()
        .expect("no output is an error");
    assert!(format!("{err:#}").contains("gh gave no error text"));

    // A body that merely starts with text is not a status line.
    assert!(gh::parse_api_output(b"not http at all", b"", Some(0)).is_err());
}

#[test]
fn verify_sha256_bytes_accepts_match_and_rejects_everything_else() {
    let payload = b"payload";
    let hex = format!("{:x}", Sha256::digest(payload));
    verify_sha256_bytes(payload, format!("{hex}  file.tar.gz\n").as_bytes(), "label").unwrap();

    let err = verify_sha256_bytes(b"tampered", hex.as_bytes(), "label").unwrap_err();
    assert!(format!("{err:#}").contains("SHA-256 checksum mismatch"));

    let err = verify_sha256_bytes(payload, b"", "my-label").unwrap_err();
    assert!(format!("{err:#}").contains("my-label"));

    let err = verify_sha256_bytes(payload, b"nothex", "label").unwrap_err();
    assert!(format!("{err:#}").contains("valid SHA-256 hex digest"));

    assert!(verify_sha256_bytes(payload, &[0xff, 0xfe], "label").is_err());
}

#[test]
fn parse_latest_release_records_tag() {
    let asset = expected_archive_name().unwrap();
    let json = release_json("v3.4.5");
    let release = super::super::network::parse_latest_release(json.into_bytes(), &asset).unwrap();
    assert_eq!(release.tag, "v3.4.5");
    assert_eq!(release.version, "3.4.5");
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn release_json(tag: &str) -> String {
    let asset = expected_archive_name().unwrap();
    format!(
        r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"assets":[
        {{"name":"{asset}","browser_download_url":"https://github.com/rysweet/amplihack-rs/releases/download/{tag}/{asset}"}},
        {{"name":"{asset}.sha256","browser_download_url":"https://github.com/rysweet/amplihack-rs/releases/download/{tag}/{asset}.sha256"}}
    ]}}"#
    )
}

struct Root {
    path: PathBuf,
}

impl Root {
    /// A private 0700 root under `$TMPDIR`.
    fn new(name: &str) -> Self {
        let base = tempfile::Builder::new()
            .prefix(&format!("amplihack-2957-{name}-"))
            .tempdir_in(std::env::temp_dir())
            .unwrap()
            .keep();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        for sub in ["bin", "home", "xdg", "ghcfg", "fixture", "empty"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(base.join(sub))
                .unwrap();
        }
        Self { path: base }
    }
    fn log(&self) -> PathBuf {
        self.path.join("gh.log")
    }
    fn gh_log(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum GhMode {
    /// auth status exits 0, api returns the release, download writes files.
    Ok,
    /// auth status exits 1.
    LoggedOut,
    /// auth status exits 0, api -i returns 403 with a reset header.
    Limited,
}

#[derive(Clone, Copy, PartialEq)]
enum Download {
    Good,
    Tampered,
    Missing,
    Symlink,
    Fail,
}

const GH_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$1 $2" in
"auth status")
  # The stand-in prints the token to both streams, as a careless gh might.
  printf 'Logged in as someone (token: %s)\n' "$GH_TOKEN"
  printf 'Logged in as someone (token: %s)\n' "$GH_TOKEN" >&2
  [ "$GH_MODE" = logged_out ] && exit 1
  exit 0 ;;
"api -i")
  if [ "$GH_MODE" = limited ]; then
    printf 'HTTP/2.0 403 Forbidden\r\nContent-Type: application/json\r\nX-Ratelimit-Reset: %s\r\n\r\n' "$GH_RESET"
    printf '{"message":"%s"}' "$GH_LIMIT_TEXT"
    exit 1
  fi
  printf 'HTTP/2.0 200 OK\r\nContent-Type: application/json\r\n\r\n'
  cat "$GH_FIXTURE/release.json"
  exit 0 ;;
"release download")
  dir=""; prev=""
  for a in "$@"; do
    [ "$prev" = "--dir" ] && dir="$a"
    prev="$a"
  done
  case "$GH_DOWNLOAD" in
  fail)
    # gh's error, with a terminal escape, then the token on a later line:
    # amplihack shows only the sanitised first line of gh's stderr.
    printf 'release not found \033[31mred\n%s\n' "$GH_TOKEN" >&2
    exit 3 ;;
  missing) exit 0 ;;
  symlink)
    ln -s "$GH_FIXTURE/archive" "$dir/$GH_ASSET"
    cp "$GH_FIXTURE/checksum" "$dir/$GH_ASSET.sha256"
    exit 0 ;;
  tampered)
    printf 'tampered' > "$dir/$GH_ASSET"
    cp "$GH_FIXTURE/checksum" "$dir/$GH_ASSET.sha256"
    exit 0 ;;
  *)
    cp "$GH_FIXTURE/archive" "$dir/$GH_ASSET"
    cp "$GH_FIXTURE/checksum" "$dir/$GH_ASSET.sha256"
    exit 0 ;;
  esac ;;
esac
echo "unexpected gh call: $*" >&2
exit 2
"#;

const PAYLOAD: &[u8] = b"archive payload for 2957";

fn stand_in_gh(root: &Root, mode: GhMode, download: Download, reset: i64) -> GhCommand {
    let script = root.path.join("bin/gh");
    std::fs::write(&script, GH_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

    let fixture = root.path.join("fixture");
    std::fs::write(fixture.join("release.json"), release_json("v9.9.9")).unwrap();
    std::fs::write(fixture.join("archive"), PAYLOAD).unwrap();
    let asset = expected_archive_name().unwrap();
    std::fs::write(
        fixture.join("checksum"),
        format!("{:x}  {asset}\n", Sha256::digest(PAYLOAD)),
    )
    .unwrap();

    let mode_text = match mode {
        GhMode::Ok => "ok",
        GhMode::LoggedOut => "logged_out",
        GhMode::Limited => "limited",
    };
    let dl_text = match download {
        Download::Good => "good",
        Download::Tampered => "tampered",
        Download::Missing => "missing",
        Download::Symlink => "symlink",
        Download::Fail => "fail",
    };
    let p = |sub: &str| root.path.join(sub).to_string_lossy().into_owned();
    GhCommand::stand_in(
        script,
        vec![
            ("PATH".into(), format!("{}:/usr/bin:/bin", p("bin"))),
            ("HOME".into(), p("home")),
            ("XDG_CONFIG_HOME".into(), p("xdg")),
            ("GH_CONFIG_DIR".into(), p("ghcfg")),
            ("GH_TOKEN".into(), SENTINEL.into()),
            ("GH_LOG".into(), root.log().to_string_lossy().into_owned()),
            ("GH_FIXTURE".into(), p("fixture")),
            ("GH_MODE".into(), mode_text.into()),
            ("GH_DOWNLOAD".into(), dl_text.into()),
            ("GH_ASSET".into(), asset),
            ("GH_RESET".into(), reset.to_string()),
            ("GH_LIMIT_TEXT".into(), RATE_LIMIT_TEXT.into()),
        ],
    )
}

/// A gh that cannot be found: the child PATH is an empty private directory.
fn missing_gh(root: &Root) -> GhCommand {
    GhCommand::stand_in(
        "gh",
        vec![(
            "PATH".into(),
            root.path.join("empty").to_string_lossy().into_owned(),
        )],
    )
}

struct StandInApi {
    url: String,
    requests: Arc<AtomicUsize>,
}

impl StandInApi {
    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// A loopback API answering every request with the same canned response. The
/// URL ends in the real releases path so the production 404 rule still applies.
fn stand_in_api(status_line: &'static str, headers: String, body: String) -> StandInApi {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let mut seen = Vec::new();
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
            counter.fetch_add(1, Ordering::SeqCst);
            let response = format!(
                "HTTP/1.1 {status_line}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    StandInApi {
        url: format!("http://127.0.0.1:{port}/repos/rysweet/amplihack-rs/releases/latest"),
        requests,
    }
}

fn ok_api(tag: &str) -> StandInApi {
    stand_in_api("200 OK", String::new(), release_json(tag))
}

fn future_reset() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 25 * 60
        + 30
}

fn assert_no_token(text: &str) {
    assert!(!text.contains(SENTINEL), "token leaked: {text}");
    assert!(
        !text.contains("Logged in"),
        "gh probe output leaked: {text}"
    );
}

fn release_for_download() -> UpdateRelease {
    let asset = expected_archive_name().unwrap();
    UpdateRelease {
        version: "9.9.9".into(),
        asset_url: format!(
            "https://github.com/rysweet/amplihack-rs/releases/download/v9.9.9/{asset}"
        ),
        checksum_url: Some(format!(
            "https://github.com/rysweet/amplihack-rs/releases/download/v9.9.9/{asset}.sha256"
        )),
        tag: "v9.9.9".into(),
    }
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

#[test]
fn logged_in_gh_handles_lookup() {
    let root = Root::new("lookup");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Good, 0);
    let api = ok_api("v0.0.1");

    let release = fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated)
        .expect("gh lookup succeeds");

    assert_eq!(release.version, "9.9.9");
    assert_eq!(release.tag, "v9.9.9");
    assert_eq!(api.count(), 0, "the stand-in API must receive no request");
    assert_eq!(
        root.gh_log(),
        vec![
            "auth status".to_string(),
            "api -i repos/rysweet/amplihack-rs/releases/latest".to_string()
        ]
    );
}

#[test]
fn no_gh_uses_anonymous_lookup() {
    let root = Root::new("nogh");
    let gh = missing_gh(&root);
    assert!(!gh.usable());
    let api = ok_api("v7.0.0");

    let release =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap();

    assert_eq!(release.version, "7.0.0");
    assert_eq!(release.tag, "v7.0.0");
    assert_eq!(api.count(), 1);
}

#[test]
fn logged_out_gh_uses_anonymous_lookup() {
    let root = Root::new("loggedout");
    let gh = stand_in_gh(&root, GhMode::LoggedOut, Download::Good, 0);
    assert!(!gh.usable());
    let _ = std::fs::remove_file(root.log());
    let api = ok_api("v7.1.0");

    let release =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap();

    assert_eq!(release.version, "7.1.0");
    assert_eq!(api.count(), 1);
    assert_eq!(root.gh_log(), vec!["auth status".to_string()]);
}

#[test]
fn anonymous_403_and_429_report_message_and_reset_without_retry() {
    for (status_line, code) in [("403 Forbidden", "403"), ("429 Too Many Requests", "429")] {
        let root = Root::new("anon-limit");
        let gh = missing_gh(&root);
        let reset = future_reset();
        let api = stand_in_api(
            status_line,
            format!("x-ratelimit-reset: {reset}\r\n"),
            format!(r#"{{"message":"{RATE_LIMIT_TEXT}"}}"#),
        );

        let err =
            fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap_err();
        let text = format!("{err:#}");

        assert!(text.contains(RATE_LIMIT_TEXT), "{text}");
        assert!(text.contains(&format!("HTTP {code}")), "{text}");
        assert!(text.contains("resets at"), "{text}");
        assert!(text.contains(" UTC (in 2"), "{text}");
        assert_eq!(api.count(), 1, "{code} must not be retried");
    }
}

#[test]
fn anonymous_403_without_reset_header_says_so() {
    let root = Root::new("anon-noreset");
    let api = stand_in_api("403 Forbidden", String::new(), "not json".into());
    let err = fetch_latest_release_with(
        &missing_gh(&root),
        &api.url,
        http_get_with_retry_unvalidated,
    )
    .unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("GitHub did not give a reset time."), "{text}");
    assert_eq!(api.count(), 1);
}

#[test]
fn gh_403_reports_message_and_reset_without_fallback_or_retry() {
    let root = Root::new("gh-403");
    let gh = stand_in_gh(&root, GhMode::Limited, Download::Good, future_reset());
    let api = ok_api("v0.0.1");

    let err =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap_err();
    let text = format!("{err:#}");

    assert!(text.contains(RATE_LIMIT_TEXT), "{text}");
    assert!(text.contains("HTTP 403"), "{text}");
    assert!(text.contains("resets at"), "{text}");
    assert!(text.contains(" UTC (in 2"), "{text}");
    assert_eq!(
        api.count(),
        0,
        "no anonymous fallback after a gh failure (D1)"
    );
    let log = root.gh_log();
    assert_eq!(log.iter().filter(|l| l.starts_with("api -i")).count(), 1);
    assert_no_token(&text);
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

#[test]
fn logged_in_gh_handles_download() {
    let root = Root::new("download");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Good, 0);

    let bytes = fetch_release_archive(&gh, &release_for_download()).unwrap();

    assert_eq!(bytes, PAYLOAD);
    let log = root.gh_log();
    let asset = expected_archive_name().unwrap();
    assert_eq!(log[0], "auth status");
    let expected_prefix = format!(
        "release download v9.9.9 --repo rysweet/amplihack-rs --pattern {asset} --pattern {asset}.sha256 --dir "
    );
    assert!(log[1].starts_with(&expected_prefix), "{log:?}");
    assert_eq!(log.len(), 2, "{log:?}");
}

#[test]
fn tampered_asset_rejected_on_gh_path() {
    let root = Root::new("tampered");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Tampered, 0);
    let err = fetch_release_archive(&gh, &release_for_download()).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("SHA-256 checksum mismatch"), "{text}");
    assert!(
        text.contains("binary download checksum verification failed"),
        "{text}"
    );
}

#[test]
fn missing_file_rejected_on_gh_path() {
    let root = Root::new("missing");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Missing, 0);
    let err = fetch_release_archive(&gh, &release_for_download()).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("did not produce"), "{text}");
    assert!(text.contains(&expected_archive_name().unwrap()), "{text}");
}

#[test]
fn symlink_rejected_on_gh_path() {
    let root = Root::new("symlink");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Symlink, 0);
    let err = fetch_release_archive(&gh, &release_for_download()).unwrap_err();
    assert!(
        format!("{err:#}").contains(&expected_archive_name().unwrap()),
        "{err:#}"
    );
}

#[test]
fn gh_download_failure_is_reported_and_not_retried_anonymously() {
    let root = Root::new("dlfail");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Fail, 0);
    let err = fetch_release_archive(&gh, &release_for_download()).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("gh release download v9.9.9 failed"), "{text}");
    assert!(text.contains("release not found"), "{text}");
    assert!(
        !text.contains('\u{1b}'),
        "terminal escapes must be stripped: {text:?}"
    );
    assert_no_token(&text);
    let downloads = root
        .gh_log()
        .iter()
        .filter(|l| l.starts_with("release download"))
        .count();
    assert_eq!(downloads, 1, "download runs once");
}

#[test]
fn hostile_tags_and_asset_names_are_rejected_before_any_gh_call() {
    let root = Root::new("hostile");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Good, 0);
    let asset = expected_archive_name().unwrap();
    let base = "https://github.com/rysweet/amplihack-rs/releases/download/v9.9.9";

    for tag in ["--repo=evil", "-x", "v1 2", "v1\n2", "v1\u{1b}"] {
        let mut release = release_for_download();
        release.tag = tag.into();
        assert!(fetch_release_archive(&gh, &release).is_err(), "tag {tag:?}");
    }
    for name in ["", "*.tar.gz", "a?b", "a[b]", "a..b", "a\u{7}b"] {
        let mut release = release_for_download();
        release.asset_url = format!("{base}/{name}");
        release.checksum_url = Some(format!("{base}/{asset}.sha256"));
        assert!(
            fetch_release_archive(&gh, &release).is_err(),
            "asset {name:?}"
        );
    }
    // No gh call may have run other than at most the probe.
    assert!(
        root.gh_log().iter().all(|l| l == "auth status"),
        "{:?}",
        root.gh_log()
    );
}

#[test]
fn non_github_download_urls_are_rejected_on_the_gh_path() {
    let root = Root::new("badurl");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Good, 0);
    let mut release = release_for_download();
    release.asset_url = "https://evil.example/amplihack.tar.gz".into();
    let err = fetch_release_archive(&gh, &release).unwrap_err();
    assert!(format!("{err:#}").contains("not from an allowed GitHub host"));
    assert!(root.gh_log().iter().all(|l| l != "release download"));
}

// ---------------------------------------------------------------------------
// Token handling
// ---------------------------------------------------------------------------

#[test]
fn sentinel_token_never_leaks() {
    // Success path.
    let root = Root::new("leak-ok");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Good, 0);
    let api = ok_api("v0.0.1");
    let release =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap();
    assert_no_token(&format!("{release:?}"));
    let _ = fetch_release_archive(&gh, &release_for_download()).unwrap();
    assert_no_token(&root.gh_log().join("\n"));

    // Tampered download error text.
    let root = Root::new("leak-tamper");
    let gh = stand_in_gh(&root, GhMode::Ok, Download::Tampered, 0);
    let err = fetch_release_archive(&gh, &release_for_download()).unwrap_err();
    assert_no_token(&format!("{err:#}"));
    assert_no_token(&root.gh_log().join("\n"));

    // Logged-out path.
    let root = Root::new("leak-out");
    let gh = stand_in_gh(&root, GhMode::LoggedOut, Download::Good, 0);
    let api = ok_api("v1.0.0");
    let release =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap();
    assert_no_token(&format!("{release:?}"));
    assert_no_token(&root.gh_log().join("\n"));

    // Rate-limited path.
    let root = Root::new("leak-403");
    let gh = stand_in_gh(&root, GhMode::Limited, Download::Good, future_reset());
    let api = ok_api("v1.0.0");
    let err =
        fetch_latest_release_with(&gh, &api.url, http_get_with_retry_unvalidated).unwrap_err();
    assert_no_token(&format!("{err:#}"));
    assert_no_token(&root.gh_log().join("\n"));
}

#[test]
fn usable_is_true_only_for_a_logged_in_gh() {
    let root = Root::new("usable");
    assert!(stand_in_gh(&root, GhMode::Ok, Download::Good, 0).usable());
    assert!(!stand_in_gh(&root, GhMode::LoggedOut, Download::Good, 0).usable());
    assert!(!missing_gh(&root).usable());
}
