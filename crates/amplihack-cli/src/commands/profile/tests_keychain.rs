//! Issue #2885: the macOS Keychain lookup (`profile::keychain`). On macOS,
//! Claude Code keeps a profile's login in the login Keychain and writes no
//! `.credentials.json`; this module asks whether an item exists, and nothing
//! else. See "Keychain logins" in docs/reference/profile-command.md.
//!
//! No test here reaches the real Keychain. Every `security` is a fake
//! script in a directory of its own, which is the child's whole `PATH`. The
//! fake appends `<HOME>|<arguments>` to a log, so a test can say exactly what
//! was run, and how often.
//!
//! Contract (`profile::keychain`):
//! - `SERVICE_PREFIX` is `Claude Code-credentials-`; `service_name(&Path)` is
//!   that prefix and the first eight lower-case hex digits of the SHA-256 of
//!   the path's UTF-8 bytes in Unicode Normalization Form C, as Claude Code
//!   computes it (crusty round 2 of PR #2891). Nothing else is done to the
//!   path: a trailing slash is kept. A path that is not UTF-8 is hashed as
//!   its raw bytes.
//! - `Keychain::{Present, Absent, Unreadable(Cause)}` and
//!   `Cause::{NoHome, NotStarted, Exit(i32), Signal, Wait}` derive `Debug`,
//!   `Clone`, `Copy`, `PartialEq` and `Eq`. `Cause`'s `Display` texts are
//!   fixed (see `cause_texts_are_fixed`).
//! - `Env` holds the platform, the home and the search path, and no state
//!   between lookups. `Env::ambient(home)` is the production value: macOS
//!   only on macOS, the process `PATH`. In test builds it reads as not macOS
//!   unless a scoped `test_seam(path)` is held on the thread;
//!   `Env::fake(macos, home, path)` builds any other value. `Env` does not
//!   implement `Debug`.
//! - `probe(&Env, service) -> Keychain` runs `security find-generic-password
//!   -s <service>`, found by bare name on the env's search path, with `HOME`
//!   set to the env's home and the three standard streams discarded. Exit 0
//!   is `Present`, 44 is `Absent`; everything else is `Unreadable`. The wait
//!   is one blocking wait until `security` ends: no deadline, no sleep and
//!   no polling loop (crusty round 2 of PR #2891). A failed wait is
//!   `Unreadable(Wait)`.
//! - `detect(&Env, dir) -> Option<Keychain>`: `None` (and nothing started)
//!   off macOS, or when `dir/.claude.json` has no identifiable
//!   `oauthAccount`; otherwise `Some(probe(env, service_name(dir)))`.
//!
//! The child's environment is cleared and given `HOME`, `PATH`, `TMPDIR`,
//! `USER` and `LOGNAME`, where set. If the measurement in the design's
//! step 3 shows `security` needs more, drop `the_child_inherits_no_api_key_variables`
//! and record the risk in the pull request.

use super::keychain::{
    Cause, Env, Keychain, PASSED, SERVICE_PREFIX, detect, probe, service_name, test_seam,
};
use super::usage_cmd::child_vars;
use crate::test_support::{EnvGuard, env_lock, write_executable};
use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{TryRecvError, channel};

/// A fake `security`: its directory (the child's whole `PATH`) and its log.
struct Fake {
    _t: tempfile::TempDir,
    dir: PathBuf,
    log: PathBuf,
    home: PathBuf,
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A fake whose script is `body`, with `LOG` replaced by the log's path.
fn fake(body: &str) -> Fake {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let dir = t.path().join("bin");
    let home = t.path().join("home");
    let log = t.path().join("security.log");
    private_dir(&dir);
    private_dir(&home);
    let script = format!(
        "#!/bin/sh\n{}",
        body.replace("LOG", &log.display().to_string())
    );
    write_executable(&dir.join("security"), &script);
    Fake {
        _t: t,
        dir,
        log,
        home,
    }
}

/// The fake exits with `code` after recording its `HOME` and arguments.
fn exits(code: i32) -> Fake {
    fake(&format!(
        "printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\nexit {code}\n"
    ))
}

impl Fake {
    fn env(&self) -> Env {
        Env::fake(true, Some(&self.home), &self.dir)
    }

    /// Whether any process still runs this fake, by its unique path.
    fn still_running(&self) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "args="])
            .output()
            .expect("ps");
        assert!(out.status.success(), "ps failed: {}", out.status);
        let script = self.dir.join("security").display().to_string();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|line| line.contains(&script))
    }

    /// The recorded lines; none when the fake was never started.
    fn lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

const SERVICE: &str = "Claude Code-credentials-d8cc6731";

// --- the service name ---------------------------------------------------------

/// The vectors were computed with `shasum -a 256` over invented literal
/// paths, not over a path from any machine, so the rule is pinned
/// independently of the code that implements it.
#[test]
fn the_service_name_is_pinned_by_independent_vectors() {
    assert_eq!(SERVICE_PREFIX, "Claude Code-credentials-");
    assert_eq!(
        service_name(Path::new("/Users/example/.amplihack/claude-profiles/work")),
        SERVICE
    );
    // A path that is not ASCII and already in NFC is hashed as its UTF-8
    // bytes, unchanged.
    let composed = Path::new(OsStr::from_bytes(
        b"/home/example/.amplihack/claude-profiles/caf\xc3\xa9",
    ));
    assert_eq!(service_name(composed), "Claude Code-credentials-4b2e2204");
}

/// Crusty round 2 of PR #2891. Claude Code 2.1.330 takes
/// `CLAUDE_CONFIG_DIR`, applies `.normalize("NFC")` and hashes the UTF-8
/// result, so a directory whose name reaches amplihack decomposed must be
/// hashed in its composed form, or a valid login reads as absent.
///
/// The vectors were computed with `shasum -a 256` over the composed bytes,
/// and checked with
/// `node -e 'crypto.createHash("sha256").update(p.normalize("NFC"))'` over
/// the decomposed text. The raw decomposed bytes hash to `1eed5f3c` and
/// `d5919c9e`, which is the wrong name.
#[test]
fn a_decomposed_path_names_the_same_item_as_its_composed_form() {
    // `cafe` and U+0301 COMBINING ACUTE ACCENT, in place of U+00E9.
    let decomposed = Path::new(OsStr::from_bytes(
        b"/home/example/.amplihack/claude-profiles/cafe\xcc\x81",
    ));
    assert_eq!(service_name(decomposed), "Claude Code-credentials-4b2e2204");
    // `A` and U+030A, then `o` and U+0308: two composed letters in one name.
    let two = Path::new(OsStr::from_bytes(
        b"/Users/example/.amplihack/claude-profiles/A\xcc\x8angstro\xcc\x88m",
    ));
    assert_eq!(service_name(two), "Claude Code-credentials-ecfd2b88");
    let composed = Path::new(OsStr::from_bytes(
        b"/Users/example/.amplihack/claude-profiles/\xc3\x85ngstr\xc3\xb6m",
    ));
    assert_eq!(service_name(composed), service_name(two));
}

#[test]
fn a_path_that_is_not_utf_8_is_hashed_as_its_raw_bytes() {
    // `printf '/tmp/x\xff' | shasum -a 256` starts f927dbd0. No such name
    // exists on a macOS file system; the rule only has to be defined.
    let raw = Path::new(OsStr::from_bytes(b"/tmp/x\xff"));
    assert_eq!(service_name(raw), "Claude Code-credentials-f927dbd0");
}

#[test]
fn the_service_name_keeps_a_trailing_slash() {
    // `printf '%s' <path>/ | shasum -a 256` starts 1a8e0b44: a trailing slash
    // is a different name, so the caller must pass the path it exports.
    // Normalization Form C changes no ASCII byte.
    assert_eq!(
        service_name(Path::new("/Users/example/.amplihack/claude-profiles/work/")),
        "Claude Code-credentials-1a8e0b44"
    );
}

#[test]
fn the_service_name_is_the_prefix_and_eight_lower_case_hex_digits() {
    for dir in [
        "/a",
        "/tmp/x/y z",
        "/Users/example/.amplihack/claude-profiles/p",
    ] {
        let s = service_name(Path::new(dir));
        let hex = s.strip_prefix(SERVICE_PREFIX).expect("the prefix");
        assert_eq!(hex.len(), 8, "{s}");
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{s}"
        );
    }
}

// --- probe: the exit status is the answer --------------------------------------

#[test]
fn exit_0_is_present_and_44_is_absent() {
    assert_eq!(probe(&exits(0).env(), SERVICE), Keychain::Present);
    assert_eq!(probe(&exits(44).env(), SERVICE), Keychain::Absent);
}

#[test]
fn any_other_exit_status_is_unreadable_and_never_a_guess() {
    // 51 is what a locked Keychain is expected to give; the design does not
    // depend on it. 1 and 45 are neighbours of 44, 255 is the largest.
    for code in [1, 2, 36, 43, 45, 51, 127, 255] {
        assert_eq!(
            probe(&exits(code).env(), SERVICE),
            Keychain::Unreadable(Cause::Exit(code)),
            "exit {code}"
        );
    }
}

#[test]
fn a_child_ended_by_a_signal_is_unreadable() {
    let f = fake("printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\nkill -9 $$\n");
    assert_eq!(
        probe(&f.env(), SERVICE),
        Keychain::Unreadable(Cause::Signal)
    );
    assert_eq!(f.lines().len(), 1, "it was started");
}

#[test]
fn a_missing_security_could_not_be_started() {
    let t = tempfile::tempdir().unwrap();
    let empty = t.path().join("empty");
    private_dir(&empty);
    let home = t.path().join("home");
    private_dir(&home);
    let env = Env::fake(true, Some(&home), &empty);
    assert_eq!(
        probe(&env, SERVICE),
        Keychain::Unreadable(Cause::NotStarted)
    );
    let env = Env::fake(true, Some(&home), &t.path().join("no-such-dir"));
    assert_eq!(
        probe(&env, SERVICE),
        Keychain::Unreadable(Cause::NotStarted)
    );
}

#[test]
fn a_security_that_is_not_executable_could_not_be_started() {
    let f = exits(0);
    let path = f.dir.join("security");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        probe(&f.env(), SERVICE),
        Keychain::Unreadable(Cause::NotStarted)
    );
    assert!(f.lines().is_empty(), "nothing ran");
}

// --- probe: exactly what is run --------------------------------------------------

#[test]
fn the_recorded_command_is_the_documented_one_and_nothing_more() {
    let f = exits(0);
    probe(&f.env(), SERVICE);
    assert_eq!(
        f.lines(),
        [format!(
            "{}|find-generic-password -s {SERVICE}",
            f.home.display()
        )],
        "HOME is the home the env was given; no -w, -g or -a"
    );
}

#[test]
fn the_child_inherits_no_api_key_variables() {
    let f = fake(
        "printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\n/usr/bin/env | /usr/bin/cut -d= -f1 | /usr/bin/sort > 'LOG.env'\nexit 0\n",
    );
    probe(&f.env(), SERVICE);
    let names = fs::read_to_string(f.log.with_extension("log.env")).unwrap();
    for name in names.lines() {
        assert!(
            !name.starts_with("ANTHROPIC")
                && !name.starts_with("CLAUDE")
                && !name.starts_with("AMPLIHACK"),
            "the child saw {name}"
        );
    }
    assert!(names.lines().any(|l| l == "HOME"), "{names}");
    assert!(names.lines().any(|l| l == "PATH"), "{names}");
}

/// Outside a test, the child's variables are the process's own values of
/// four fixed names, read by `usage_cmd::child_vars`: never an API-key
/// variable, and never `HOME`, which is the home the env was given. The
/// reader is tried on invented names, so no test changes `PATH`.
#[test]
fn the_ambient_child_is_given_four_named_variables_and_no_others() {
    assert_eq!(PASSED, ["PATH", "TMPDIR", "USER", "LOGNAME"]);
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::set([
        ("AMPLIHACK_TEST_2885_SECOND", "two"),
        ("AMPLIHACK_TEST_2885_FIRST", "one"),
        ("AMPLIHACK_TEST_2885_EMPTY", ""),
        ("AMPLIHACK_TEST_2885_OTHER", "CANARY-LEAK-PROBE"),
    ]);
    let got = child_vars(&[
        "AMPLIHACK_TEST_2885_FIRST",
        "AMPLIHACK_TEST_2885_EMPTY",
        "AMPLIHACK_TEST_2885_UNSET",
        "AMPLIHACK_TEST_2885_SECOND",
    ]);
    let shown: Vec<(&str, &str)> = got
        .iter()
        .map(|(name, value)| (*name, value.to_str().unwrap()))
        .collect();
    assert_eq!(
        shown,
        [
            ("AMPLIHACK_TEST_2885_FIRST", "one"),
            ("AMPLIHACK_TEST_2885_SECOND", "two")
        ],
        "the named ones in the order asked; unset and empty ones left out"
    );
}

#[test]
fn nothing_the_child_prints_reaches_the_answer_or_the_test_output() {
    // The fake prints an account name and a secret on every stream; the
    // streams are discarded, so the answer is the exit status alone.
    let f = fake(
        "printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\necho 'acct=\"user-SENTINEL-KEYCHAIN\"'\necho 'password: SENTINEL-SECRET' >&2\nexit 0\n",
    );
    let got = probe(&f.env(), SERVICE);
    assert_eq!(got, Keychain::Present);
    let shown = format!("{got:?}");
    assert!(!shown.contains("SENTINEL"), "{shown}");
}

// --- probe: no home, no start ------------------------------------------------------

#[test]
fn no_absolute_home_means_unreadable_and_nothing_is_started() {
    let f = exits(0);
    let none = Env::fake(true, None, &f.dir);
    assert_eq!(probe(&none, SERVICE), Keychain::Unreadable(Cause::NoHome));
    let relative = Env::fake(true, Some(Path::new("home")), &f.dir);
    assert_eq!(
        probe(&relative, SERVICE),
        Keychain::Unreadable(Cause::NoHome)
    );
    let empty = Env::fake(true, Some(Path::new("")), &f.dir);
    assert_eq!(probe(&empty, SERVICE), Keychain::Unreadable(Cause::NoHome));
    assert!(f.lines().is_empty(), "security was never started");
}

// --- probe: one blocking wait, no deadline (crusty round 2 of PR #2891) ------------

/// The lookup holds until `security` ends, however long that takes, and
/// answers from its exit status then. The test needs no clock: the fake
/// blocks reading a FIFO, so it cannot end before the test writes to it.
///
/// The test opens the FIFO for writing, which returns only once a reader has
/// it open. In the passing case that reader is the fake, which then waits
/// for the line, so `probe` cannot have answered yet. If `probe` answers
/// without waiting (the fake was not started, say), the thread that ran it
/// sends that answer and then opens the FIFO read-write, so the test's open
/// returns and finds the early answer, rather than blocking forever.
#[test]
fn the_lookup_waits_for_security_to_end_with_no_time_limit_of_its_own() {
    let f = fake("printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\nread line < 'LOG.fifo'\nexit 0\n");
    let fifo = f.log.with_extension("log.fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo failed: {made}");
    let (tx, rx) = channel();
    let (home, dir, unblock) = (f.home.clone(), f.dir.clone(), fifo.clone());
    let lookup = std::thread::spawn(move || {
        let got = probe(&Env::fake(true, Some(&home), &dir), SERVICE);
        tx.send(got).unwrap();
        // Lets the test's open return if no fake ever opened the FIFO.
        let _ = fs::OpenOptions::new().read(true).write(true).open(&unblock);
    });
    let mut writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    assert_eq!(
        rx.try_recv(),
        Err(TryRecvError::Empty),
        "the lookup answered while security was still running"
    );
    assert_eq!(f.lines().len(), 1, "security was started once");
    writer.write_all(b"go\n").unwrap();
    drop(writer);
    assert_eq!(rx.recv().unwrap(), Keychain::Present);
    lookup.join().unwrap();
    // The one wait also reaped the child.
    assert!(!f.still_running(), "the fake is still running");
}

/// The lookup keeps no state between calls: an `Env` whose first lookup
/// gave no answer still asks `security` the next time, so the order of the
/// lookups never changes an answer.
#[test]
fn an_unreadable_answer_does_not_change_the_next_lookup() {
    let f = fake(
        "printf '%s|%s\\n' \"$HOME\" \"$*\" >> 'LOG'\ncase \"$3\" in\n  'Claude Code-credentials-00000000') exit 51 ;;\n  *) exit 0 ;;\nesac\n",
    );
    let env = f.env();
    assert_eq!(
        probe(&env, "Claude Code-credentials-00000000"),
        Keychain::Unreadable(Cause::Exit(51))
    );
    assert_eq!(probe(&env, SERVICE), Keychain::Present);
    assert_eq!(f.lines().len(), 2, "each lookup started its own security");
}

// --- detect --------------------------------------------------------------------------

const ACCOUNT: &str = r#"{"oauthAccount":{"accountUuid":"uuid-SENTINEL-KEYCHAIN","emailAddress":"user-SENTINEL-KEYCHAIN@example.com"}}"#;

fn profile_dir(t: &Path, claude_json: Option<&str>) -> PathBuf {
    let dir = t.join("profiles").join("work");
    private_dir(&dir);
    if let Some(body) = claude_json {
        let path = dir.join(".claude.json");
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    dir
}

#[test]
fn detect_asks_for_the_directorys_own_service_when_there_is_an_account() {
    let f = exits(0);
    let dir = profile_dir(f.home.parent().unwrap(), Some(ACCOUNT));
    assert_eq!(detect(&f.env(), &dir), Some(Keychain::Present));
    assert_eq!(
        f.lines(),
        [format!(
            "{}|find-generic-password -s {}",
            f.home.display(),
            service_name(&dir)
        )]
    );
    // Absent and unreadable pass through unchanged.
    let absent = exits(44);
    let dir = profile_dir(absent.home.parent().unwrap(), Some(ACCOUNT));
    assert_eq!(detect(&absent.env(), &dir), Some(Keychain::Absent));
    let locked = exits(51);
    let dir = profile_dir(locked.home.parent().unwrap(), Some(ACCOUNT));
    assert_eq!(
        detect(&locked.env(), &dir),
        Some(Keychain::Unreadable(Cause::Exit(51)))
    );
}

#[test]
fn detect_asks_nothing_off_macos() {
    let f = exits(0);
    let dir = profile_dir(f.home.parent().unwrap(), Some(ACCOUNT));
    let env = Env::fake(false, Some(&f.home), &f.dir);
    assert_eq!(detect(&env, &dir), None);
    assert!(f.lines().is_empty(), "a recording fake was never started");
}

#[test]
fn detect_asks_nothing_without_an_identifiable_account() {
    for (why, body) in [
        ("no .claude.json", None),
        ("corrupt", Some("{not json")),
        ("no oauthAccount", Some(r#"{"numStartups":3}"#)),
        ("empty block", Some(r#"{"oauthAccount":{}}"#)),
        ("not an object", Some(r#"{"oauthAccount":"someone"}"#)),
        (
            "empty values",
            Some(r#"{"oauthAccount":{"accountUuid":"","emailAddress":""}}"#),
        ),
        (
            "not a string",
            Some(r#"{"oauthAccount":{"accountUuid":7}}"#),
        ),
    ] {
        let f = exits(0);
        let dir = profile_dir(f.home.parent().unwrap(), body);
        assert_eq!(detect(&f.env(), &dir), None, "{why}");
        assert!(f.lines().is_empty(), "{why}: security was started");
    }
}

#[test]
fn detect_fails_loudly_when_no_home_is_known() {
    let f = exits(0);
    let dir = profile_dir(f.home.parent().unwrap(), Some(ACCOUNT));
    let env = Env::fake(true, None, &f.dir);
    assert_eq!(
        detect(&env, &dir),
        Some(Keychain::Unreadable(Cause::NoHome))
    );
    assert!(f.lines().is_empty());
}

// --- the texts ---------------------------------------------------------------------------

#[test]
fn cause_texts_are_fixed() {
    assert_eq!(Cause::NoHome.to_string(), "no home directory is known");
    assert_eq!(
        Cause::NotStarted.to_string(),
        "`security` could not be started"
    );
    assert_eq!(Cause::Exit(51).to_string(), "`security` exit status 51");
    assert_eq!(Cause::Exit(1).to_string(), "`security` exit status 1");
    assert_eq!(
        Cause::Signal.to_string(),
        "`security` was ended by a signal"
    );
    assert_eq!(
        Cause::Wait.to_string(),
        "`security` could not be waited for"
    );
}

// --- the test seam stays a test seam -------------------------------------------------------

#[test]
fn the_ambient_env_is_not_macos_in_a_test_build_unless_the_seam_is_held() {
    let t = tempfile::tempdir().unwrap();
    assert!(!Env::ambient(Some(t.path())).macos(), "no seam: not macOS");
    let f = exits(0);
    {
        let _seam = test_seam(&f.dir);
        let env = Env::ambient(Some(&f.home));
        assert!(env.macos());
        assert_eq!(probe(&env, SERVICE), Keychain::Present, "the seam's PATH");
        assert_eq!(f.lines().len(), 1);
    }
    assert!(!Env::ambient(Some(t.path())).macos(), "restored on drop");
}

#[test]
fn the_seam_belongs_to_one_thread() {
    let f = exits(0);
    let _seam = test_seam(&f.dir);
    let other = std::thread::spawn(|| Env::ambient(None).macos())
        .join()
        .unwrap();
    assert!(
        !other,
        "a thread the code under test starts reads not macOS"
    );
}

// --- source scans: what the module can and cannot do -----------------------------------------

const MODULE: &str = include_str!("keychain.rs");

#[test]
fn the_module_starts_one_command_and_keeps_every_stream_closed() {
    assert_eq!(MODULE.matches("Command::new(").count(), 1);
    assert_eq!(MODULE.matches(".spawn(").count(), 1);
    assert_eq!(
        MODULE.matches("Stdio::null()").count(),
        3,
        "stdin, stdout, stderr"
    );
    for banned in [
        ".output(",
        ".status(",
        "Stdio::piped",
        "Stdio::inherit",
        "\"-w\"",
        "\"-g\"",
        "\"-a\"",
        "\"-D\"",
        "\"-j\"",
        "env::var",
        "var_os",
        "\"accountUuid\"",
        "\"organizationUuid\"",
        "unwrap_or(Keychain::Present",
    ] {
        assert!(!MODULE.contains(banned), "keychain.rs contains {banned}");
    }
    assert!(MODULE.contains("find-generic-password"));
}

/// Crusty round 2 of PR #2891: the lookup sets no wall-clock limit, never
/// sleeps and never polls. The child is waited for once, by a blocking
/// `wait`, and never killed: no code path here sends a signal.
#[test]
fn the_module_has_no_deadline_no_sleep_no_polling_and_no_kill() {
    let code: String = MODULE
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [
        "try_wait",
        "sleep",
        "Instant",
        "Duration",
        "recv_timeout",
        "wait_timeout",
        ".kill(",
        "deadline",
    ] {
        assert!(!code.contains(banned), "keychain.rs code contains {banned}");
    }
    assert_eq!(code.matches(".wait()").count(), 1, "one blocking wait");
}

/// Only `keychain.rs` knows the command. A second place that builds it is a
/// second place that can get it wrong (a `-w` that reads the secret).
#[test]
fn no_other_source_file_under_profile_names_the_command() {
    fn walk(dir: &Path, hits: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "testdata" {
                    walk(&path, hits);
                }
                continue;
            }
            let is_test = name.starts_with("tests") || name == "keychain.rs";
            if !is_test
                && path.extension().is_some_and(|e| e == "rs")
                && fs::read_to_string(&path)
                    .unwrap()
                    .contains("find-generic-password")
            {
                hits.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile");
    let mut hits = Vec::new();
    walk(&root, &mut hits);
    assert!(hits.is_empty(), "{hits:?}");
}
