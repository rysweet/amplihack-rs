//! Issue #1526 — the launch bootstrap is an implicit installer too.
//!
//! `ensure_framework_installed` runs on every interactive `launch`, `claude`
//! and `copilot`. Whenever `~/.amplihack/.claude` looks incomplete it runs a
//! full install, then tops up the slash commands and auto-repairs the hook
//! registrations in `~/.claude/settings.json` — regardless of which binary is
//! asking, and regardless of `AMPLIHACK_SKIP_AUTO_INSTALL`. With a stamp newer
//! than the running binary it must return before any of that.
//!
//! The warning goes to stderr at most once per process. Startup self-heal
//! normally prints it first, so the bootstrap stays quiet; when self-heal was
//! skipped (a `-h` or `--version` forwarded to the launched tool) the
//! bootstrap prints it instead. With the bypass set it prints nothing. Its
//! tracing stays at `debug`: the binary's `fmt` subscriber writes to stdout,
//! and must stay quiet below any `RUST_LOG=warn` a user might set.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::{
    IsolatedHome, args, assert_home_is, assert_refusal_warning, changes, counting_installer,
    newer_version, snapshot,
};
use super::*;
use std::cell::Cell;
use std::fs;

/// Run `f` under the binary's `fmt` subscriber at `RUST_LOG=warn` and return
/// what it would print. In the real binary that output goes to stdout, which
/// guard paths must keep clean (SR-4).
fn printed_at_warn<T>(f: impl FnOnce() -> T) -> (T, String) {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, f);
    let printed = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    (out, printed)
}

/// No staging tree at all: on an older binary the bootstrap would run a full
/// install. A missing hooks binary is planted as a tripwire, so falling
/// through fails fast instead of deploying anything.
///
/// Startup self-heal has not run in this "process", as for
/// `amplihack claude -- --version`, so the bootstrap prints the warning
/// itself: once, and not at all under the bypass.
#[test]
fn bootstrap_with_a_newer_stamp_stages_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let fixture = tempfile::tempdir().unwrap();
    let mut env = IsolatedHome::new(home);
    env.set(
        "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
        fixture.path().join("no-such-amplihack-hooks"),
    );
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");

    let newer = newer_version();
    version_stamp::write_installed_version(&newer).unwrap();
    let before = snapshot(home);

    assert_home_is(home);
    crate::self_heal::set_downgrade_warned(false);
    let mut notice = Vec::new();
    let (result, printed) = printed_at_warn(|| ensure_framework_installed_with(&mut notice));
    result.unwrap_or_else(|e| {
        panic!(
            "v{} under a v{newer} stamp: the bootstrap must refuse, not fail the launch: {e:#}",
            crate::VERSION
        )
    });
    assert_eq!(
        printed, "",
        "the bootstrap refusal logs at debug: at RUST_LOG=warn the binary would print \
         this to stdout, and the warning goes to stderr"
    );
    assert_refusal_warning(&String::from_utf8_lossy(&notice), home, &newer);

    let mut again = Vec::new();
    ensure_framework_installed_with(&mut again).expect("a repeat refusal is not a failure");
    assert_eq!(
        String::from_utf8_lossy(&again),
        "",
        "the warning is printed at most once per process"
    );

    crate::self_heal::set_downgrade_warned(false);
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    let mut bypassed = Vec::new();
    ensure_framework_installed_with(&mut bypassed).expect("a bypassed refusal is not a failure");
    assert_eq!(
        String::from_utf8_lossy(&bypassed),
        "",
        "under AMPLIHACK_SKIP_AUTO_INSTALL the bootstrap refuses without a warning"
    );

    assert_eq!(
        changes(&before, &snapshot(home)),
        Vec::<String>::new(),
        "over a v{newer} install the bootstrap must change nothing under HOME: no staged \
         assets, no binaries, no ~/.claude, the stamp untouched"
    );
}

/// A complete staging tree and a `settings.json` without amplihack hooks: on
/// an older binary the bootstrap would skip the install but still restage the
/// slash commands and auto-repair `settings.json` (with a backup). A real
/// hooks stub is provided so that repair is reachable.
///
/// This is the `amplihack claude` order: startup self-heal refuses and warns
/// first, then the bootstrap refuses without a second warning.
#[test]
fn bootstrap_with_a_newer_stamp_leaves_settings_without_hooks_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let fixture = tempfile::tempdir().unwrap();
    let hooks_stub = create_exe_stub(&fixture.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::new(home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");

    create_minimal_staged_assets(home);
    let settings = home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let sentinel: &[u8] = b"{\n  \"model\": \"sentinel\"\n}\n";
    fs::write(&settings, sentinel).unwrap();
    let newer = newer_version();
    version_stamp::write_installed_version(&newer).unwrap();
    let before = snapshot(home);

    assert_home_is(home);
    crate::self_heal::set_downgrade_warned(false);
    let calls = Cell::new(0u32);
    let mut self_heal_notice = Vec::new();
    crate::self_heal::ensure_assets_match_binary_version_with(
        &args(&["amplihack", "claude"]),
        &mut self_heal_notice,
        counting_installer(&calls),
    )
    .expect("a downgrade refusal is not a self-heal failure");
    assert_eq!(calls.get(), 0, "self-heal must not install over v{newer}");
    let mut notice = Vec::new();
    ensure_framework_installed_with(&mut notice)
        .expect("a downgrade refusal is not a launch failure");
    assert_refusal_warning(&String::from_utf8_lossy(&self_heal_notice), home, &newer);
    assert_eq!(
        String::from_utf8_lossy(&notice),
        "",
        "self-heal already warned in this process, so the bootstrap must not repeat it"
    );

    assert_eq!(
        changes(&before, &snapshot(home)),
        Vec::<String>::new(),
        "an older binary must change nothing under HOME: settings.json byte-identical, \
         no backups, no slash commands restaged"
    );
}

/// S5 (SR-8): a settings.json that does not parse is never "repaired". With a
/// stamp that is not newer the bootstrap reaches the #202 check; it must warn
/// and leave the file byte-identical, with no backup beside it.
#[test]
fn bootstrap_leaves_unparseable_settings_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let fixture = tempfile::tempdir().unwrap();
    let hooks_stub = create_exe_stub(&fixture.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::new(home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");

    create_minimal_staged_assets(home);
    let claude = home.join(".claude");
    let settings = claude.join("settings.json");
    fs::create_dir_all(&claude).unwrap();
    let corrupt: &[u8] = b"{ \"model\": \"sentinel\", ";
    fs::write(&settings, corrupt).unwrap();
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    assert_home_is(home);
    ensure_framework_installed_with(&mut Vec::new())
        .expect("an unparseable settings.json warns; it does not fail the launch");

    assert_eq!(
        fs::read(&settings).unwrap(),
        corrupt,
        "settings.json was rewritten"
    );
    let backups: Vec<String> = fs::read_dir(&claude)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("settings.json") && name != "settings.json")
        .collect();
    assert_eq!(
        backups,
        Vec::<String>::new(),
        "no backup of a file left unchanged"
    );
}
