//! Tests for `amplihack workstreams status` (issue #1710).
//!
//! Written before the implementation: they define its contract. No test here
//! reads the real `~/.claude`, `~/.amplihack`, `~/.config/anthropic` or
//! `~/.claude-profiles`, runs tmux against the user's server, or calls GitHub.
//! Every external source is a fake passed through [`Sources`]; every path is a
//! `tempfile` fixture.
//!
//! The interface these tests assume (all `pub(crate)` unless noted):
//!
//! - `meta`: `RunMeta { schema, session, parent_session_id, parent_run,
//!   recipe, started_at, cwd, branch, issue_number, pr_number }` (serde,
//!   `Default`), `sanitize_id(&str) -> Option<String>`,
//!   `write_atomic(&Path, &RunMeta) -> Result<()>`, `read(&Path) -> Option<RunMeta>`.
//! - `logptr`: `read(&Path) -> LogInfo { issue, pr, started, exit_code, tail }`.
//! - `autodrive`: `parse_layout(&str) -> Result<Layout>`, `layout() -> &'static Layout`,
//!   `state_root(Option<&OsStr>, &Path) -> PathBuf`,
//!   `state_key(Option<&str>, &str) -> String`, `read(&Path) -> Option<AutoDriveState>`,
//!   `AutoDriveState { phases, crusty_rounds, merge_ready_rounds, concerns }` (`Default`).
//! - `sources`: traits `Clock`, `LiveSessions`, `PrSource`, `GitOrigin`,
//!   `UsageSource`; structs `LiveSession`, `Pr` (deserialises `gh pr list
//!   --json` output), `ProfileInfo`, `UsageStream`, `Sources`.
//! - `stage`: `PrLookup`, `RunFacts` (`Default`), `Marker`, `Status`,
//!   `find_pr`, `status`, `format_elapsed`.
//! - `manifest`: `ManifestRow`, `parse`, `merge`, `Entry`.
//! - `discover`: `Scope`, `scope_from_env`, `Run` (`Default`, `Clone`),
//!   `Discovery { runs, warnings }`, `discover(&Path, &Scope) -> Discovery`.
//! - `logins`: `LoginCell`, `LoginRow`, `is_api_key_profile(&Path) -> bool`,
//!   `cell_from(&UsageReport) -> LoginCell`, `collect(UsageStream, Instant) -> Vec<LoginRow>`.
//! - `render`: `WIDTH`, `bar(u8, usize)`, `fit(&str, usize)`, `WorkRow`,
//!   `Report`, `render(&Report) -> String`.
//! - `profile::usage_parse` becomes `pub(crate)` so these tests can build
//!   `UsageReport` fixtures; `WorkstreamsStatusArgs` is re-exported at the
//!   crate root like the other subcommand args.
//! - `mod`: `run_with(&WorkstreamsStatusArgs, &Sources, &dyn Fn(&str) ->
//!   Option<String>, &mut dyn Write, &mut dyn Write) -> i32`.

use super::autodrive::{self, AutoDriveState};
use super::discover::{self, Run, Scope};
use super::logins::{self, LoginCell, LoginRow};
use super::manifest::{self, ManifestRow};
use super::meta::{self, RunMeta};
use super::render::{self, Report, WIDTH, WorkRow};
use super::sources::{
    Clock, GitOrigin, LiveSession, LiveSessions, Pr, PrSource, ProfileInfo, Sources, UsageSource,
    UsageStream,
};
use super::stage::{self, Marker, PrLookup, RunFacts};
use super::{logptr, run_with};
use crate::WorkstreamsStatusArgs;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- fixtures

const NOW_RFC: &str = "2026-10-27T14:05:00Z";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(NOW_RFC).unwrap().to_utc()
}

fn mins_ago(m: i64) -> DateTime<Utc> {
    now() - chrono::Duration::minutes(m)
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A private recipe-log directory inside a fresh temp dir.
fn log_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("recipe-logs");
    private_dir(&dir);
    (tmp, dir)
}

#[derive(Default, Clone)]
struct MetaFixture<'a> {
    parent_session_id: Option<&'a str>,
    parent_run: Option<&'a str>,
    issue: Option<u64>,
    pr: Option<u64>,
    branch: Option<&'a str>,
    cwd: Option<&'a str>,
    started: Option<DateTime<Utc>>,
}

/// Write `<session>.log` and `<session>.meta.json` the way `detach.rs` does.
fn add_run(dir: &Path, session: &str, m: MetaFixture<'_>) {
    private_file(&dir.join(format!("{session}.log")), "");
    let meta = serde_json::json!({
        "schema": 1,
        "session": session,
        "parent_session_id": m.parent_session_id,
        "parent_run": m.parent_run,
        "recipe": "auto-drive-to-merge",
        "started_at": m.started.unwrap_or_else(|| mins_ago(12)).to_rfc3339(),
        "cwd": m.cwd,
        "branch": m.branch,
        "issue_number": m.issue,
        "pr_number": m.pr,
    });
    private_file(
        &dir.join(format!("{session}.meta.json")),
        &serde_json::to_string(&meta).unwrap(),
    );
}

/// A run started before meta files existed: a log only.
fn add_legacy_log(dir: &Path, session: &str, body: &str) {
    private_file(&dir.join(format!("{session}.log")), body);
}

fn pointer(json: serde_json::Value) -> String {
    format!("amplihack.recipe.log_pointer {json}\n")
}

fn sessions(runs: &[Run]) -> Vec<String> {
    let mut v: Vec<String> = runs.iter().map(|r| r.session.clone()).collect();
    v.sort();
    v
}

fn gh_prs(json: &str) -> Vec<Pr> {
    serde_json::from_str(json).expect("Pr deserialises gh pr list --json output")
}

fn pr_json(number: u64, state: &str, mergeable: &str, head: &str, title: &str) -> String {
    format!(
        r#"{{"number":{number},"title":"{title}","state":"{state}","mergeable":"{mergeable}","headRefName":"{head}","baseRefName":"main"}}"#
    )
}

fn one_pr(number: u64, state: &str, mergeable: &str) -> Pr {
    gh_prs(&format!(
        "[{}]",
        pr_json(number, state, mergeable, "feat/x", "t")
    ))
    .pop()
    .unwrap()
}

fn report(state: UsageState, session: Option<f64>, week: Option<f64>) -> UsageReport {
    let window = |label: &str, p: f64| UsageWindow {
        label: label.to_string(),
        used_percentage: p,
        resets_text: Some("4pm".to_string()),
        resets_at: None,
    };
    UsageReport {
        state,
        session: session.map(|p| window("session", p)),
        week_all: week.map(|p| window("all models", p)),
        week_models: Vec::new(),
    }
}

// ------------------------------------------------------------------- fakes

struct FixedClock;
impl Clock for FixedClock {
    fn now(&self) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .from_utc_datetime(&now().naive_utc())
    }
}

#[derive(Default)]
struct FakeLive(Vec<LiveSession>);
impl LiveSessions for FakeLive {
    fn list(&self) -> Vec<LiveSession> {
        self.0.clone()
    }
}

#[derive(Default)]
struct FakePrs {
    prs: Option<Vec<Pr>>,
    calls: AtomicUsize,
}
impl PrSource for FakePrs {
    fn list(&self, _cwd: &Path) -> Result<Vec<Pr>, ()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prs.clone().ok_or(())
    }
}

struct FakeGit;
impl GitOrigin for FakeGit {
    fn origin(&self, _cwd: &Path) -> Option<String> {
        Some("https://github.com/o/r.git".to_string())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum UsageMode {
    Immediate,
    Never,
}

struct FakeUsage {
    mode: UsageMode,
    started: AtomicBool,
    cancelled: Arc<AtomicBool>,
    // Keeps the sender alive in Never mode so the receiver never disconnects.
    held: Mutex<Vec<mpsc::Sender<(String, UsageReport)>>>,
}
impl FakeUsage {
    fn new(mode: UsageMode) -> Self {
        Self {
            mode,
            started: AtomicBool::new(false),
            cancelled: Arc::default(),
            held: Mutex::default(),
        }
    }
}
impl UsageSource for FakeUsage {
    fn start(&self, _timeout: Duration) -> anyhow::Result<UsageStream> {
        self.started.store(true, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        if self.mode == UsageMode::Immediate {
            tx.send((
                "main".into(),
                report(UsageState::Ok, Some(72.0), Some(41.0)),
            ))
            .unwrap();
        }
        self.held.lock().unwrap().push(tx);
        let cancelled = self.cancelled.clone();
        Ok(UsageStream {
            profiles: vec![ProfileInfo {
                name: "main".into(),
                live: true,
                api_key: false,
            }],
            results: rx,
            cancel: Box::new(move || cancelled.store(true, Ordering::SeqCst)),
        })
    }
}

fn sources<'a>(
    log_dir: &Path,
    live: &'a FakeLive,
    prs: &'a FakePrs,
    usage: &'a FakeUsage,
) -> Sources<'a> {
    Sources {
        clock: &FixedClock,
        live,
        prs,
        git: &FakeGit,
        usage,
        log_dir: Some(log_dir.to_path_buf()),
        state_root: Some(log_dir.with_file_name("state").join("auto-drive")),
        default_manifest: Some(log_dir.with_file_name("no-such-workstreams.tsv")),
    }
}

fn args(
    all: bool,
    manifest: Option<PathBuf>,
    no_logins: bool,
    deadline: Option<u64>,
) -> WorkstreamsStatusArgs {
    WorkstreamsStatusArgs {
        all,
        manifest,
        no_logins,
        logins_deadline: deadline,
    }
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |k| {
        pairs
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| (*v).to_string())
    }
}

fn run_cmd(
    a: &WorkstreamsStatusArgs,
    src: &Sources<'_>,
    env: &dyn Fn(&str) -> Option<String>,
) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with(a, src, env, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn assert_plain_and_narrow(text: &str) {
    for line in text.lines() {
        assert!(
            line.chars().count() <= WIDTH,
            "line wider than {WIDTH}: {line:?}"
        );
        assert!(
            !line.chars().any(char::is_control),
            "control character in {line:?}"
        );
    }
    assert!(!text.contains('\x1b'));
}

// =================================================================== stage

fn facts() -> RunFacts {
    RunFacts {
        started: Some(mins_ago(12)),
        ..RunFacts::default()
    }
}

fn ad(
    phases: &[&str],
    crusty: u32,
    merge_ready: u32,
    concerns: Option<u32>,
) -> Option<AutoDriveState> {
    Some(AutoDriveState {
        phases: phases.iter().map(|s| s.to_string()).collect(),
        crusty_rounds: crusty,
        merge_ready_rounds: merge_ready,
        concerns,
    })
}

#[test]
fn queued_row_is_zero_percent_and_says_queued_once() {
    let s = stage::status(
        &RunFacts {
            queued: true,
            ..RunFacts::default()
        },
        now(),
    );
    assert_eq!(s.pct, 0);
    assert_eq!(s.marker, Marker::Waiting);
    assert_eq!(s.text, "queued");
}

#[test]
fn a_live_run_with_no_pr_is_building() {
    let s = stage::status(
        &RunFacts {
            live: true,
            ..facts()
        },
        now(),
    );
    assert_eq!((s.pct, s.marker), (15, Marker::Live));
    assert_eq!(s.text, "building, 12m");
}

#[test]
fn a_stopped_run_with_no_known_cause_says_waiting_or_stopped() {
    let s = stage::status(&facts(), now());
    assert_eq!((s.pct, s.marker), (15, Marker::Waiting));
    assert_eq!(s.text, "building, (waiting or stopped), 12m");
}

#[test]
fn an_open_pr_is_thirty_percent() {
    let s = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Found(one_pr(1715, "OPEN", "MERGEABLE")),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 30);
    assert_eq!(s.text, "PR open, 12m");
}

#[test]
fn build_phase_only_enters_the_review_loop_not_pr_open() {
    // The prototype's off-by-one: phases.tsv lists COMPLETED phases, so a
    // recorded `build` means the review loop has started.
    let s = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Found(one_pr(1715, "OPEN", "MERGEABLE")),
            autodrive: ad(&["build"], 0, 0, None),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 40);
    assert!(s.text.starts_with("review loop"), "{}", s.text);
}

#[test]
fn review_rounds_add_four_percent_each_and_cap_at_sixty() {
    for (rounds, pct) in [(1, 44), (2, 48), (5, 60), (9, 60)] {
        let s = stage::status(
            &RunFacts {
                live: true,
                autodrive: ad(&["build"], rounds, 0, None),
                ..facts()
            },
            now(),
        );
        assert_eq!(s.pct, pct, "rounds {rounds}");
        assert!(
            s.text.starts_with(&format!("review round {rounds}")),
            "{}",
            s.text
        );
    }
}

#[test]
fn review_rounds_alone_without_build_phase_count_as_review_loop() {
    let s = stage::status(
        &RunFacts {
            autodrive: ad(&[], 1, 0, None),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 44);
}

#[test]
fn concerns_follow_the_stage_only_when_a_review_round_exists() {
    let with = stage::status(
        &RunFacts {
            live: true,
            autodrive: ad(&["build"], 2, 0, Some(3)),
            ..facts()
        },
        now(),
    );
    assert_eq!(with.text, "review round 2, 3 concerns, 12m");
    let without = stage::status(
        &RunFacts {
            live: true,
            autodrive: ad(&["build"], 0, 0, Some(3)),
            ..facts()
        },
        now(),
    );
    assert!(!without.text.contains("concerns"), "{}", without.text);
}

#[test]
fn crusty_loop_phase_enters_the_merge_ready_loop() {
    let s = stage::status(
        &RunFacts {
            live: true,
            autodrive: ad(&["build", "crusty-loop"], 3, 0, None),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 70);
    assert!(s.text.starts_with("merge-ready loop"), "{}", s.text);
}

#[test]
fn merge_ready_rounds_add_five_percent_each_and_cap_at_ninety_five() {
    for (rounds, pct) in [(1, 75), (3, 85), (5, 95), (12, 95)] {
        let s = stage::status(
            &RunFacts {
                live: true,
                autodrive: ad(&["build", "crusty-loop"], 2, rounds, None),
                ..facts()
            },
            now(),
        );
        assert_eq!(s.pct, pct, "rounds {rounds}");
        assert!(
            s.text.starts_with(&format!("merge-ready round {rounds}")),
            "{}",
            s.text
        );
    }
}

#[test]
fn merge_ready_rounds_without_the_crusty_phase_still_count() {
    let s = stage::status(
        &RunFacts {
            autodrive: ad(&["build"], 2, 1, None),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 75);
}

#[test]
fn merge_ready_loop_phase_is_ninety_five_until_merged() {
    let s = stage::status(
        &RunFacts {
            autodrive: ad(&["build", "crusty-loop", "merge-ready-loop"], 2, 2, None),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 95);
}

#[test]
fn merged_phase_or_merged_pr_is_one_hundred_with_no_marker() {
    let by_phase = stage::status(
        &RunFacts {
            autodrive: ad(
                &["build", "crusty-loop", "merge-ready-loop", "merged"],
                2,
                2,
                None,
            ),
            started: Some(now() - chrono::Duration::hours(27)),
            ..RunFacts::default()
        },
        now(),
    );
    assert_eq!((by_phase.pct, by_phase.marker), (100, Marker::Blank));
    assert_eq!(by_phase.text, "merged, 1d3h");

    let by_pr = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Found(one_pr(1715, "MERGED", "UNKNOWN")),
            ..facts()
        },
        now(),
    );
    assert_eq!((by_pr.pct, by_pr.marker), (100, Marker::Blank));
    assert!(by_pr.text.starts_with("merged"), "{}", by_pr.text);
}

#[test]
fn a_closed_pr_says_pr_closed_and_keeps_the_percentage() {
    let s = stage::status(
        &RunFacts {
            pr: PrLookup::Found(one_pr(1715, "CLOSED", "UNKNOWN")),
            exit_code: Some(0),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.pct, 30);
    assert!(s.text.starts_with("PR closed"), "{}", s.text);
}

#[test]
fn conflicts_name_the_base_branch_and_default_to_main() {
    let mut pr = gh_prs(
        r#"[{"number":9,"title":"t","state":"OPEN","mergeable":"CONFLICTING","headRefName":"x","baseRefName":"develop"}]"#,
    )
    .pop()
    .unwrap();
    let s = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Found(pr.clone()),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.text, "PR open, conflicts with develop, 12m");

    pr = gh_prs(
        r#"[{"number":9,"title":"t","state":"OPEN","mergeable":"CONFLICTING","headRefName":"x"}]"#,
    )
    .pop()
    .unwrap();
    let s = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Found(pr),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.text, "PR open, conflicts with main, 12m");
}

#[test]
fn an_unknown_pr_lookup_says_so_instead_of_guessing() {
    let s = stage::status(
        &RunFacts {
            live: true,
            pr: PrLookup::Unknown,
            ..facts()
        },
        now(),
    );
    assert!(s.text.contains("PR state unknown"), "{}", s.text);
    assert!(!s.text.contains("PR open"));
    assert!(!s.text.contains("conflicts"));
}

#[test]
fn a_usage_limit_in_the_log_tail_is_the_reason() {
    let s = stage::status(
        &RunFacts {
            log_tail: b"...\nYou've hit your session limit \xc2\xb7 resets 4pm (UTC)\n".to_vec(),
            exit_code: Some(1),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.marker, Marker::Waiting);
    assert!(s.text.contains("stopped on a usage limit"), "{}", s.text);
    assert!(
        !s.text.contains("failed"),
        "the limit wins over the exit code"
    );
    assert!(
        !s.text.contains("resets"),
        "log text never reaches the output"
    );
}

#[test]
fn exit_codes_give_failed_or_finished() {
    let failed = stage::status(
        &RunFacts {
            exit_code: Some(3),
            ..facts()
        },
        now(),
    );
    assert_eq!(failed.text, "building, failed (exit 3), 12m");
    let finished = stage::status(
        &RunFacts {
            exit_code: Some(0),
            ..facts()
        },
        now(),
    );
    assert_eq!(finished.text, "building, finished, 12m");
}

#[test]
fn a_live_run_never_shows_a_stop_reason() {
    let s = stage::status(
        &RunFacts {
            live: true,
            exit_code: Some(1),
            ..facts()
        },
        now(),
    );
    assert_eq!(s.marker, Marker::Live);
    assert!(!s.text.contains("failed"), "{}", s.text);
}

#[test]
fn markers_render_as_the_documented_glyphs() {
    assert_eq!(Marker::Live.glyph(), '▶');
    assert_eq!(Marker::Waiting.glyph(), '⏸');
    assert_eq!(Marker::Blank.glyph(), ' ');
}

#[test]
fn elapsed_is_formatted_from_the_start_time() {
    let at = |d: chrono::Duration| stage::format_elapsed(Some(now() - d), now());
    assert_eq!(at(chrono::Duration::seconds(30)), "<1m");
    assert_eq!(at(chrono::Duration::minutes(12)), "12m");
    assert_eq!(at(chrono::Duration::minutes(102)), "1h42m");
    assert_eq!(at(chrono::Duration::minutes(65)), "1h05m");
    assert_eq!(at(chrono::Duration::hours(27)), "1d3h");
}

#[test]
fn elapsed_is_a_question_mark_when_unknown_or_in_the_future() {
    assert_eq!(stage::format_elapsed(None, now()), "?");
    assert_eq!(
        stage::format_elapsed(Some(now() + chrono::Duration::minutes(5)), now()),
        "?"
    );
    // No overflow panic on extreme values.
    assert!(!stage::format_elapsed(Some(DateTime::<Utc>::MIN_UTC), now()).is_empty());
}

// ----------------------------------------------------------------- find_pr

fn prs_fixture() -> Vec<Pr> {
    gh_prs(&format!(
        "[{},{},{},{}]",
        pr_json(
            1715,
            "OPEN",
            "MERGEABLE",
            "feat/issue-1710-x",
            "feat: thing (#1710)"
        ),
        pr_json(1716, "OPEN", "MERGEABLE", "feat/other", "fix #17100 typo"),
        pr_json(1704, "MERGED", "UNKNOWN", "feat/rot", "rotator (#1700)"),
        pr_json(1720, "OPEN", "MERGEABLE", "feat/end", "ends with #1698"),
    ))
}

fn found(l: PrLookup) -> Option<u64> {
    match l {
        PrLookup::Found(pr) => Some(pr.number),
        _ => None,
    }
}

#[test]
fn find_pr_prefers_meta_then_pointer_then_branch_then_title() {
    let prs = prs_fixture();
    let p = Some(prs.as_slice());
    assert_eq!(
        found(stage::find_pr(
            p,
            Some(1704),
            Some(1716),
            Some("feat/issue-1710-x"),
            Some(1710)
        )),
        Some(1704)
    );
    assert_eq!(
        found(stage::find_pr(
            p,
            None,
            Some(1716),
            Some("feat/issue-1710-x"),
            Some(1710)
        )),
        Some(1716)
    );
    assert_eq!(
        found(stage::find_pr(
            p,
            None,
            None,
            Some("feat/issue-1710-x"),
            Some(999)
        )),
        Some(1715)
    );
    assert_eq!(
        found(stage::find_pr(p, None, None, Some("nope"), Some(1700))),
        Some(1704)
    );
}

#[test]
fn title_match_needs_a_non_digit_or_end_after_the_issue() {
    let prs = prs_fixture();
    let p = Some(prs.as_slice());
    // "#17100" must not match issue 1710; "(#1710)" must.
    assert_eq!(
        found(stage::find_pr(p, None, None, None, Some(1710))),
        Some(1715)
    );
    // "#1698" at the end of the title.
    assert_eq!(
        found(stage::find_pr(p, None, None, None, Some(1698))),
        Some(1720)
    );
    assert!(matches!(
        stage::find_pr(p, None, None, None, Some(171)),
        PrLookup::None
    ));
}

#[test]
fn find_pr_is_unknown_when_the_pr_list_failed() {
    assert!(matches!(
        stage::find_pr(None, None, None, Some("b"), Some(1710)),
        PrLookup::Unknown
    ));
}

#[test]
fn a_pr_number_absent_from_the_list_is_not_found() {
    let prs = prs_fixture();
    assert!(matches!(
        stage::find_pr(Some(prs.as_slice()), Some(4242), None, None, None),
        PrLookup::None
    ));
}

// ================================================================== render

#[test]
fn bars_fill_one_cell_per_five_percent_and_clamp() {
    assert_eq!(render::bar(0, 20), "░".repeat(20));
    assert_eq!(
        render::bar(48, 20),
        format!("{}{}", "█".repeat(9), "░".repeat(11))
    );
    assert_eq!(render::bar(100, 20), "█".repeat(20));
    assert_eq!(render::bar(250, 20), "█".repeat(20));
    assert_eq!(
        render::bar(72, 10),
        format!("{}{}", "█".repeat(7), "░".repeat(3))
    );
    assert_eq!(render::bar(4, 20), "░".repeat(20));
}

#[test]
fn fit_pads_or_truncates_with_an_ellipsis_by_chars() {
    assert_eq!(render::fit("ab", 4), "ab  ");
    assert_eq!(render::fit("abcd", 4), "abcd");
    assert_eq!(render::fit("abcdef", 4), "abc…");
    assert_eq!(render::fit("ééééé", 4), "ééé…");
    assert_eq!(render::fit("", 0), "");
}

fn work_row(
    label: &str,
    issue: Option<u64>,
    pr: Option<u64>,
    pct: u8,
    marker: Marker,
    text: &str,
) -> WorkRow {
    WorkRow {
        label: label.to_string(),
        issue,
        pr,
        status: stage::Status {
            pct,
            marker,
            text: text.to_string(),
        },
    }
}

fn sample_report() -> Report {
    Report {
        now: FixedClock.now(),
        note: None,
        rows: vec![
            work_row(
                "Workstream status skill",
                Some(1710),
                Some(1715),
                48,
                Marker::Live,
                "review round 2, 3 concerns, 1h42m",
            ),
            work_row(
                "A label that is far too long to fit in its column",
                Some(1698),
                Some(1716),
                30,
                Marker::Waiting,
                "PR open, conflicts with main, stopped on a usage limit, 3h10m",
            ),
            work_row(
                "Docs cleanup",
                Some(1712),
                None,
                0,
                Marker::Waiting,
                "queued",
            ),
            work_row(
                "Remaining step outputs",
                Some(1684),
                Some(1688),
                100,
                Marker::Blank,
                "merged, 1d3h",
            ),
        ],
        logins: Some(vec![
            LoginRow {
                profile: "main".into(),
                live: true,
                cell: LoginCell::Reading {
                    session: Some(72),
                    week: Some(41),
                    state: "ok".into(),
                    reset: Some("session resets 4pm, week resets Nov 2".into()),
                },
            },
            LoginRow {
                profile: "a-profile-name-longer-than-its-field".into(),
                live: true,
                cell: LoginCell::NoReading("timeout".into()),
            },
            LoginRow {
                profile: "console".into(),
                live: false,
                cell: LoginCell::ApiKey,
            },
        ]),
    }
}

#[test]
fn render_has_the_header_sections_and_rules() {
    let text = render::render(&sample_report());
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "WORKSTREAMS  2026-10-27 14:05");
    assert!(text.contains("\nLOGINS\n"), "{text}");
    let rule = format!("  {}", "─".repeat(98));
    assert_eq!(text.matches(&rule).count(), 2, "{text}");
    assert_plain_and_narrow(&text);
}

#[test]
fn workstream_rows_use_the_documented_columns() {
    let text = render::render(&sample_report());
    let row = text
        .lines()
        .find(|l| l.contains("Workstream status skill"))
        .unwrap();
    let c: Vec<char> = row.chars().collect();
    let span = |a: usize, b: usize| c[a..b].iter().collect::<String>();
    assert_eq!(span(0, 2), "  ");
    assert_eq!(span(2, 30), render::fit("Workstream status skill", 28));
    assert_eq!(span(30, 36), "#1710 ");
    assert_eq!(span(36, 42), "#1715 ");
    assert_eq!(span(42, 62), render::bar(48, 20));
    assert_eq!(span(62, 67), "  48%");
    assert_eq!(c[68], '▶');
    assert!(span(70, c.len()).starts_with("review round 2, 3 concerns"));
}

#[test]
fn long_labels_and_stage_text_are_truncated_to_fit_100_columns() {
    let text = render::render(&sample_report());
    let row = text.lines().find(|l| l.contains("A label that")).unwrap();
    // The label column is 28 wide including its trailing space.
    assert!(row.contains("A label that is far too lo… #1698"), "{row}");
    assert!(row.trim_end().ends_with('…'), "stage text truncated: {row}");
    assert!(row.chars().count() <= 100);
}

#[test]
fn missing_issue_or_pr_shows_a_dash_and_merged_rows_have_no_marker() {
    let text = render::render(&sample_report());
    let queued = text.lines().find(|l| l.contains("Docs cleanup")).unwrap();
    let c: Vec<char> = queued.chars().collect();
    assert_eq!(c[36..42].iter().collect::<String>(), "-     ");
    assert_eq!(c[68], '⏸');
    let merged = text.lines().find(|l| l.contains("Remaining step")).unwrap();
    let c: Vec<char> = merged.chars().collect();
    assert_eq!(c[62..67].iter().collect::<String>(), " 100%");
    assert_eq!(c[68], ' ');
}

#[test]
fn login_rows_show_two_bars_no_reading_or_api_key() {
    let text = render::render(&sample_report());
    let main = text
        .lines()
        .find(|l| l.starts_with("  main (live)"))
        .unwrap();
    assert!(
        main.contains(&format!("S {}  72%", render::bar(72, 10))),
        "{main}"
    );
    assert!(
        main.contains(&format!("W {}  41%", render::bar(41, 10))),
        "{main}"
    );
    assert!(main.contains("ok"));
    let long = text
        .lines()
        .find(|l| l.contains("no reading (timeout)"))
        .unwrap();
    assert!(
        long.contains(" (live)"),
        "(live) survives a long name: {long}"
    );
    assert!(!long.contains('█') && !long.contains('░'));
    let api = text.lines().find(|l| l.contains("console")).unwrap();
    assert!(
        api.contains("API key, billed per token (no plan limits)"),
        "{api}"
    );
    assert!(!api.contains('░'));
}

#[test]
fn no_logins_section_when_logins_are_skipped() {
    let mut r = sample_report();
    r.logins = None;
    assert!(!render::render(&r).contains("LOGINS"));
}

#[test]
fn the_no_session_note_replaces_the_workstream_table() {
    let mut r = sample_report();
    r.rows.clear();
    r.note = Some(
        "no amplihack session id in this environment; run with --all to list every run on the host"
            .into(),
    );
    let text = render::render(&r);
    assert!(text.contains("no amplihack session id in this environment"));
    assert!(text.contains("LOGINS"));
    assert_plain_and_narrow(&text);
}

#[test]
fn hostile_text_is_sanitised_before_it_is_rendered() {
    let mut r = sample_report();
    r.rows = vec![work_row(
        "evil\x1b[31mred\u{202e}rtl\nnext",
        Some(1),
        None,
        15,
        Marker::Live,
        "building\x07",
    )];
    if let Some(l) = r.logins.as_mut() {
        l[0].profile = "pro\x1bfile\u{9b}".into();
    }
    let text = render::render(&r);
    assert_plain_and_narrow(&text);
    assert!(!text.contains('\u{202e}'));
    assert!(!text.contains('\u{9b}'));
    assert!(
        text.contains("evil?[31mred?rtl?next") || text.contains("evil?[31mred?rtl?n"),
        "{text}"
    );
}

#[test]
fn every_line_stays_within_100_columns_for_long_fixtures() {
    let mut r = sample_report();
    for i in 0..20 {
        r.rows.push(work_row(
            &"x".repeat(200 + i),
            Some(u64::MAX),
            Some(u64::MAX),
            100,
            Marker::Waiting,
            &"y".repeat(300),
        ));
    }
    if let Some(l) = r.logins.as_mut() {
        l.push(LoginRow {
            profile: "p".repeat(300),
            live: true,
            cell: LoginCell::Reading {
                session: Some(100),
                week: Some(100),
                state: "session_exhausted".repeat(10),
                reset: Some("r".repeat(300)),
            },
        });
    }
    assert_plain_and_narrow(&render::render(&r));
}

// ================================================================ manifest

#[test]
fn manifest_parses_tab_separated_rows_and_skips_comments() {
    let (rows, warnings) = manifest::parse(
        "# label\tissue\tpr\n\nWorkstream status skill\t#1710\t-\nLogin rotator\t1700\t#1704\n",
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        rows,
        vec![
            ManifestRow {
                label: "Workstream status skill".into(),
                issue: Some(1710),
                pr: None,
            },
            ManifestRow {
                label: "Login rotator".into(),
                issue: Some(1700),
                pr: Some(1704),
            },
        ]
    );
}

#[test]
fn bad_manifest_lines_warn_with_their_line_number_and_are_skipped() {
    let (rows, warnings) =
        manifest::parse("ok\t1\t-\nonly two\t5\nbad\tabc\t-\nbad pr\t2\t#x\n\t3\t-\n");
    assert_eq!(rows.len(), 1);
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    for (w, n) in warnings.iter().zip([2, 3, 4, 5]) {
        assert!(w.starts_with(&format!("manifest:{n}: ")), "{w}");
    }
}

#[test]
fn manifest_warnings_clip_and_sanitise_bad_values() {
    let bad = format!("\x1b[31m{}", "9".repeat(10) + &"z".repeat(100));
    let (_, warnings) = manifest::parse(&format!("l\t{bad}\t-\n"));
    assert_eq!(warnings.len(), 1);
    assert!(!warnings[0].contains('\x1b'));
    assert!(warnings[0].chars().count() < 120, "{}", warnings[0]);
}

#[test]
fn manifest_labels_keep_spaces_and_lose_control_characters() {
    let (rows, _) = manifest::parse("  My label\x1b here \t7\t-\n");
    assert_eq!(rows[0].label, "My label? here");
}

fn run(session: &str, issue: Option<u64>, pr: Option<u64>, started: Option<DateTime<Utc>>) -> Run {
    Run {
        session: session.into(),
        recipe: Some("auto-drive-to-merge".into()),
        issue,
        pr,
        started,
        ..Run::default()
    }
}

#[test]
fn merge_matches_pr_before_issue_and_uses_each_run_once() {
    let manifest = vec![
        ManifestRow {
            label: "by issue".into(),
            issue: Some(1710),
            pr: None,
        },
        ManifestRow {
            label: "by pr".into(),
            issue: Some(1),
            pr: Some(1715),
        },
        ManifestRow {
            label: "queued".into(),
            issue: Some(1712),
            pr: None,
        },
    ];
    let runs = vec![
        run("a", Some(1710), Some(1715), Some(mins_ago(30))),
        run("b", Some(1710), None, Some(mins_ago(20))),
        run("c", Some(1800), None, Some(mins_ago(50))),
        run("d", None, None, None),
        run("e", Some(1801), None, Some(mins_ago(90))),
    ];
    let merged = manifest::merge(manifest, runs);
    let view: Vec<(String, Option<String>)> = merged
        .iter()
        .map(|e| (e.label.clone(), e.run.as_ref().map(|r| r.session.clone())))
        .collect();
    assert_eq!(
        view,
        vec![
            // "by pr" claims run a first, so "by issue" gets run b.
            ("by issue".into(), Some("b".into())),
            ("by pr".into(), Some("a".into())),
            ("queued".into(), None),
            // Discovered-only rows: oldest first, unknown start last.
            ("auto-drive-to-merge #1801".into(), Some("e".into())),
            ("auto-drive-to-merge #1800".into(), Some("c".into())),
            ("d".into(), Some("d".into())),
        ]
    );
}

#[test]
fn merge_without_a_manifest_keeps_every_run() {
    let merged = manifest::merge(
        Vec::new(),
        vec![run("a", Some(1), None, None), run("b", Some(1), None, None)],
    );
    assert_eq!(merged.len(), 2, "two runs for one issue are both shown");
}

// ================================================================ discover

#[test]
fn scope_prefers_the_detached_run_over_the_session_id() {
    let get = env_of(&[
        ("AMPLIHACK_DETACHED_SESSION", "amp-run-1"),
        ("AMPLIHACK_SESSION_ID", "sess-1"),
    ]);
    assert_eq!(
        discover::scope_from_env(false, &get),
        Scope::Run("amp-run-1".into())
    );
    let get = env_of(&[("AMPLIHACK_SESSION_ID", "sess-1")]);
    assert_eq!(
        discover::scope_from_env(false, &get),
        Scope::Session("sess-1".into())
    );
    assert_eq!(discover::scope_from_env(false, &env_of(&[])), Scope::None);
    assert_eq!(discover::scope_from_env(true, &get), Scope::All);
}

#[test]
fn an_invalid_id_counts_as_unset() {
    let get = env_of(&[
        ("AMPLIHACK_DETACHED_SESSION", "../evil"),
        ("AMPLIHACK_SESSION_ID", "sess\n1"),
    ]);
    assert_eq!(discover::scope_from_env(false, &get), Scope::None);
}

#[test]
fn session_scope_lists_children_and_their_descendants_only() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "a",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "b",
        MetaFixture {
            parent_run: Some("a"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "c",
        MetaFixture {
            parent_run: Some("b"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "d",
        MetaFixture {
            parent_session_id: Some("other"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "e",
        MetaFixture {
            parent_run: Some("d"),
            ..Default::default()
        },
    );
    let found = discover::discover(&dir, &Scope::Session("S".into()));
    assert_eq!(sessions(&found.runs), ["a", "b", "c"]);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
}

#[test]
fn run_scope_matches_parent_run_only_and_excludes_siblings_and_root() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "root",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "child",
        MetaFixture {
            parent_session_id: Some("S"),
            parent_run: Some("root"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "grandchild",
        MetaFixture {
            parent_session_id: Some("S"),
            parent_run: Some("child"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "sibling",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    let found = discover::discover(&dir, &Scope::Run("root".into()));
    assert_eq!(sessions(&found.runs), ["child", "grandchild"]);
}

#[test]
fn a_parent_run_cycle_terminates() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "a",
        MetaFixture {
            parent_session_id: Some("S"),
            parent_run: Some("c"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "b",
        MetaFixture {
            parent_run: Some("a"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "c",
        MetaFixture {
            parent_run: Some("b"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "x",
        MetaFixture {
            parent_run: Some("y"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "y",
        MetaFixture {
            parent_run: Some("x"),
            ..Default::default()
        },
    );
    let found = discover::discover(&dir, &Scope::Session("S".into()));
    assert_eq!(sessions(&found.runs), ["a", "b", "c"]);
}

#[test]
fn runs_without_meta_are_listed_only_with_all() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "new",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    add_legacy_log(&dir, "old", "");
    assert_eq!(
        sessions(&discover::discover(&dir, &Scope::Session("S".into())).runs),
        ["new"]
    );
    assert_eq!(
        sessions(&discover::discover(&dir, &Scope::All).runs),
        ["new", "old"]
    );
}

#[test]
fn spec_and_context_files_are_not_runs() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "r",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    private_file(&dir.join("r.spec.json"), "{}");
    private_file(&dir.join("r.context.json"), "{}");
    assert_eq!(sessions(&discover::discover(&dir, &Scope::All).runs), ["r"]);
}

#[test]
fn meta_fields_reach_the_run() {
    let (_t, dir) = log_dir();
    let cwd = dir.parent().unwrap().to_str().unwrap().to_string();
    add_run(
        &dir,
        "r",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1710),
            pr: Some(1715),
            branch: Some("feat/issue-1710-x"),
            cwd: Some(&cwd),
            started: Some(mins_ago(42)),
            ..Default::default()
        },
    );
    let r = discover::discover(&dir, &Scope::Session("S".into()))
        .runs
        .pop()
        .unwrap();
    assert_eq!((r.issue, r.pr), (Some(1710), Some(1715)));
    assert_eq!(r.branch.as_deref(), Some("feat/issue-1710-x"));
    assert_eq!(r.cwd.as_deref(), Some(Path::new(&cwd)));
    assert_eq!(r.started, Some(mins_ago(42)));
}

#[test]
fn a_relative_or_missing_meta_cwd_is_dropped() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "rel",
        MetaFixture {
            parent_session_id: Some("S"),
            cwd: Some("relative/dir"),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "gone",
        MetaFixture {
            parent_session_id: Some("S"),
            cwd: Some("/no/such/dir/for/test"),
            ..Default::default()
        },
    );
    for r in discover::discover(&dir, &Scope::Session("S".into())).runs {
        assert_eq!(r.cwd, None, "{}", r.session);
    }
}

#[test]
fn legacy_start_time_comes_from_the_log_pointer_never_the_mtime() {
    let (_t, dir) = log_dir();
    add_legacy_log(
        &dir,
        "with-pointer",
        &pointer(
            serde_json::json!({"schema_version":1,"event":"early","timestamp":"2026-10-27T13:00:00Z","issue_number":"1710"}),
        ),
    );
    add_legacy_log(&dir, "bare", "just output\n");
    let runs = discover::discover(&dir, &Scope::All).runs;
    let get = |s: &str| runs.iter().find(|r| r.session == s).unwrap().clone();
    assert_eq!(get("with-pointer").started, Some(mins_ago(65)));
    assert_eq!(get("with-pointer").issue, Some(1710));
    assert_eq!(
        get("bare").started,
        None,
        "the file mtime is never a start time"
    );
}

#[test]
fn an_unsafe_log_dir_gives_one_warning_and_no_runs() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "r",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)).unwrap();
    let found = discover::discover(&dir, &Scope::All);
    assert!(found.runs.is_empty());
    assert_eq!(found.warnings.len(), 1, "{:?}", found.warnings);
    // The reader never chmods.
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o770
    );
}

#[test]
fn a_symlinked_log_dir_is_not_followed() {
    let (t, dir) = log_dir();
    add_run(
        &dir,
        "r",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    let link = t.path().join("link");
    symlink(&dir, &link).unwrap();
    let found = discover::discover(&link, &Scope::All);
    assert!(found.runs.is_empty());
    assert_eq!(found.warnings.len(), 1);
}

#[test]
fn a_missing_log_dir_is_silent_and_not_created() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("absent");
    let found = discover::discover(&dir, &Scope::All);
    assert!(found.runs.is_empty() && found.warnings.is_empty());
    assert!(!dir.exists(), "the reader never creates the log dir");
}

#[test]
fn an_unsafe_meta_file_counts_as_no_meta() {
    let (t, dir) = log_dir();
    add_run(
        &dir,
        "loose",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    fs::set_permissions(
        dir.join("loose.meta.json"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    add_legacy_log(&dir, "linked", "");
    let target = t.path().join("elsewhere.json");
    private_file(
        &target,
        r#"{"schema":1,"session":"linked","parent_session_id":"S"}"#,
    );
    symlink(&target, dir.join("linked.meta.json")).unwrap();
    assert!(
        discover::discover(&dir, &Scope::Session("S".into()))
            .runs
            .is_empty()
    );
}

#[test]
fn a_meta_file_naming_another_session_is_ignored() {
    let (_t, dir) = log_dir();
    add_legacy_log(&dir, "real", "");
    private_file(
        &dir.join("real.meta.json"),
        r#"{"schema":1,"session":"other","parent_session_id":"S"}"#,
    );
    assert!(
        discover::discover(&dir, &Scope::Session("S".into()))
            .runs
            .is_empty()
    );
}

// ==================================================================== meta

#[test]
fn ids_are_sanitised_to_the_safe_alphabet() {
    assert_eq!(
        meta::sanitize_id("amp-run_1.2:3").as_deref(),
        Some("amp-run_1.2:3")
    );
    for bad in ["", "a/b", "a\nb", "a\x1bb", "a b", "é", &"x".repeat(129)] {
        assert_eq!(meta::sanitize_id(bad), None, "{bad:?}");
    }
    assert!(meta::sanitize_id(&"x".repeat(128)).is_some());
}

fn sample_meta() -> RunMeta {
    RunMeta {
        schema: 1,
        session: "amp-r-1".into(),
        parent_session_id: Some("sess-1".into()),
        parent_run: None,
        recipe: Some("auto-drive-to-merge".into()),
        started_at: Some(mins_ago(1)),
        cwd: Some(PathBuf::from("/w")),
        branch: Some("feat/x".into()),
        issue_number: Some(1710),
        pr_number: None,
    }
}

#[test]
fn meta_round_trips_through_a_private_atomic_write() {
    let (_t, dir) = log_dir();
    let path = dir.join("amp-r-1.meta.json");
    meta::write_atomic(&path, &sample_meta()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(meta::read(&path), Some(sample_meta()));
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != "amp-r-1.meta.json")
        .collect();
    assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
}

#[test]
fn meta_holds_exactly_the_documented_keys() {
    let value = serde_json::to_value(sample_meta()).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "branch",
            "cwd",
            "issue_number",
            "parent_run",
            "parent_session_id",
            "pr_number",
            "recipe",
            "schema",
            "session",
            "started_at"
        ]
    );
}

#[test]
fn meta_read_accepts_unknown_fields_and_missing_nullables() {
    let (_t, dir) = log_dir();
    let path = dir.join("m.meta.json");
    private_file(&path, r#"{"schema":1,"session":"m","future_field":true}"#);
    let m = meta::read(&path).unwrap();
    assert_eq!(m.session, "m");
    assert_eq!(m.parent_session_id, None);
}

#[test]
fn meta_read_refuses_unsafe_or_oversized_files() {
    let (t, dir) = log_dir();
    let ok = r#"{"schema":1,"session":"m"}"#;
    let wrong_schema = dir.join("a.meta.json");
    private_file(&wrong_schema, r#"{"schema":2,"session":"m"}"#);
    assert_eq!(meta::read(&wrong_schema), None);

    let loose = dir.join("b.meta.json");
    private_file(&loose, ok);
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(meta::read(&loose), None);

    let big = dir.join("c.meta.json");
    private_file(
        &big,
        &format!(
            r#"{{"schema":1,"session":"m","pad":"{}"}}"#,
            "x".repeat(17 * 1024)
        ),
    );
    assert_eq!(meta::read(&big), None);

    let target = t.path().join("t.json");
    private_file(&target, ok);
    let link = dir.join("d.meta.json");
    symlink(&target, &link).unwrap();
    assert_eq!(meta::read(&link), None);

    let garbage = dir.join("e.meta.json");
    private_file(&garbage, "not json");
    assert_eq!(meta::read(&garbage), None);
}

#[test]
fn write_atomic_never_follows_a_planted_symlink() {
    let (t, dir) = log_dir();
    let victim = t.path().join("victim");
    private_file(&victim, "untouched");
    let path = dir.join("amp-r-1.meta.json");
    symlink(&victim, &path).unwrap();
    let _ = meta::write_atomic(&path, &sample_meta());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched");
}

// ================================================================== logptr

#[test]
fn log_pointer_gives_issue_pr_start_and_exit() {
    let (_t, dir) = log_dir();
    let log = dir.join("r.log");
    let body = [
        pointer(serde_json::json!({"event":"early","timestamp":"2026-10-27T13:00:00Z","issue_number":"#1710","pr_number":1715,"task_description":"SENTINEL-TASK-TEXT"})),
        "lots of output\n".repeat(10),
        pointer(serde_json::json!({"event":"final","timestamp":"2026-10-27T14:00:00Z","exit_code":2,"status":"failure"})),
    ]
    .concat();
    private_file(&log, &body);
    let info = logptr::read(&log);
    assert_eq!(info.issue, Some(1710));
    assert_eq!(info.pr, Some(1715));
    assert_eq!(info.started, Some(mins_ago(65)));
    assert_eq!(info.exit_code, Some(2));
    assert!(
        !format!("{info:?}").contains("SENTINEL-TASK-TEXT"),
        "task text is never kept"
    );
}

#[test]
fn log_pointer_tolerates_bad_numbers_and_missing_events() {
    let (_t, dir) = log_dir();
    let log = dir.join("r.log");
    private_file(
        &log,
        &pointer(
            serde_json::json!({"event":"early","issue_number":"99999999999999999999999","pr_number":"abc","timestamp":"nope"}),
        ),
    );
    let info = logptr::read(&log);
    assert_eq!(
        (info.issue, info.pr, info.started, info.exit_code),
        (None, None, None, None)
    );
    let empty = dir.join("e.log");
    private_file(&empty, "");
    assert_eq!(logptr::read(&empty).exit_code, None);
}

#[test]
fn log_pointer_reads_only_the_head_and_tail_of_a_big_log() {
    let (_t, dir) = log_dir();
    let log = dir.join("big.log");
    let early = pointer(serde_json::json!({"event":"early","timestamp":"2026-10-27T13:00:00Z"}));
    let fin = pointer(serde_json::json!({"event":"final","exit_code":0}));
    let middle_ptr = pointer(serde_json::json!({"event":"final","exit_code":9}));
    let filler = "z".repeat(1024) + "\n";
    let body = format!(
        "{early}{}{middle_ptr}{}{fin}",
        filler.repeat(200),
        filler.repeat(200)
    );
    private_file(&log, &body);
    let info = logptr::read(&log);
    assert_eq!(
        info.exit_code,
        Some(0),
        "the last final event in the tail wins"
    );
    assert!(info.tail.len() <= 64 * 1024);
}

// =============================================================== autodrive

fn workspace_root() -> PathBuf {
    let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !cur.join("amplifier-bundle").is_dir() {
        assert!(cur.pop(), "no workspace root");
    }
    cur
}

#[test]
fn the_compiled_in_layout_has_the_shared_names() {
    let l = autodrive::layout();
    assert_eq!(l.state_subdir, "auto-drive");
    assert_eq!(l.phases_file, "phases.tsv");
    assert_eq!(l.phase_build, "build");
    assert_eq!(l.phase_crusty_loop, "crusty-loop");
    assert_eq!(l.phase_merge_ready_loop, "merge-ready-loop");
    assert_eq!(l.phase_merged, "merged");
    assert_eq!(l.loop_crusty, "crusty");
    assert_eq!(l.loop_merge_ready, "merge-ready");
    assert_eq!(l.latest_suffix, "-latest.json");
    assert_eq!(l.records_suffix, "-records.tsv");
    assert_eq!(l.round_infix, "-round-");
}

#[test]
fn the_layout_file_is_eleven_plain_assignments() {
    let text =
        fs::read_to_string(workspace_root().join("amplifier-bundle/tools/autodrive_layout.sh"))
            .unwrap();
    let re = regex::Regex::new(r"^AUTODRIVE_[A-Z_]+=[a-z.-]+$").unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    assert_eq!(lines.len(), 11, "{lines:?}");
    for l in lines {
        assert!(re.is_match(l), "not a plain NAME=value line: {l:?}");
    }
    assert_eq!(
        autodrive::parse_layout(&text).unwrap(),
        *autodrive::layout()
    );
}

#[test]
fn parse_layout_rejects_anything_but_plain_values() {
    let good =
        fs::read_to_string(workspace_root().join("amplifier-bundle/tools/autodrive_layout.sh"))
            .unwrap();
    for bad in [
        "AUTODRIVE_STATE_SUBDIR=$(id)",
        "AUTODRIVE_STATE_SUBDIR=`id`",
        "AUTODRIVE_STATE_SUBDIR=a;b",
        "AUTODRIVE_STATE_SUBDIR=a b",
        "AUTODRIVE_STATE_SUBDIR=\"auto-drive\"",
        "export AUTODRIVE_STATE_SUBDIR=auto-drive",
    ] {
        let text = format!("{good}\n{bad}\n");
        assert!(autodrive::parse_layout(&text).is_err(), "{bad:?} accepted");
    }
    let missing: String = good
        .lines()
        .filter(|l| !l.starts_with("AUTODRIVE_PHASE_MERGED="))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        autodrive::parse_layout(&missing).is_err(),
        "a missing name is an error"
    );
}

#[test]
fn the_bash_writers_source_the_layout_instead_of_literals() {
    let tools = workspace_root().join("amplifier-bundle/tools");
    for script in ["autodrive_state.sh", "autodrive_loop.sh"] {
        let text = fs::read_to_string(tools.join(script)).unwrap();
        assert!(
            text.contains("autodrive_layout.sh"),
            "{script} does not source the layout"
        );
        assert!(
            text.contains("BASH_SOURCE"),
            "{script} must find the layout through BASH_SOURCE"
        );
    }
    let state = fs::read_to_string(tools.join("autodrive_state.sh")).unwrap();
    assert!(
        !state.contains("'%s/auto-drive\\n'"),
        "the state root literal must use the layout"
    );
    assert!(state.contains("ERROR: autodrive_layout.sh not found"));
}

#[test]
fn recipe_loop_names_are_layout_loop_names() {
    let l = autodrive::layout();
    let re = regex::Regex::new(r#"--loop-name\s+"?([a-z-]+)"?"#).unwrap();
    let dir = workspace_root().join("amplifier-bundle/recipes");
    let mut seen = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with("autodrive-") && name.ends_with(".yaml")) {
            continue;
        }
        for c in re.captures_iter(&fs::read_to_string(&path).unwrap()) {
            seen += 1;
            let v = &c[1];
            assert!(
                v == l.loop_crusty || v == l.loop_merge_ready,
                "{name}: --loop-name {v}"
            );
        }
    }
    assert!(
        seen > 0,
        "no --loop-name found; the check is not checking anything"
    );
}

#[test]
fn state_key_matches_the_bash_rule() {
    assert_eq!(
        autodrive::state_key(Some("https://github.com/o/r.git"), "1715"),
        "https___github.com_o_r_1715"
    );
    assert_eq!(
        autodrive::state_key(Some("git@github.com:o/r"), "feat/x"),
        "git_github.com_o_r_feat_x"
    );
    assert_eq!(autodrive::state_key(None, "1715"), "local_1715");
    assert_eq!(
        autodrive::state_key(Some("https://h/é r"), "1"),
        "https___h____r_1",
        "byte-wise like tr"
    );
}

#[test]
fn state_key_agrees_with_autodrive_state_sh() {
    let have = |b: &str| {
        std::process::Command::new(b)
            .arg("--version")
            .output()
            .is_ok()
    };
    if !have("bash") || !have("git") {
        eprintln!("skipped: bash or git not installed");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    private_dir(&home);
    let script = workspace_root().join("amplifier-bundle/tools/autodrive_state.sh");
    // Other tests point PATH at fake binaries while they hold the env lock;
    // read the real one under it.
    let path = {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::var_os("PATH").unwrap_or_default()
    };
    let clean = |cmd: &mut std::process::Command| {
        cmd.env_clear()
            .env("PATH", &path)
            .env("HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null());
    };
    for (origin, ident) in [
        (Some("https://github.com/o/r.git"), "1715"),
        (Some("git@github.com:o/r.git"), "feat/issue-1710-x"),
        (Some("ssh://h/p a/r"), "x y"),
        (None, "17"),
    ] {
        let repo = t
            .path()
            .join(format!("repo-{ident}").replace(['/', ' '], "_"));
        private_dir(&repo);
        let mut init = amplihack_git::command();
        clean(&mut init);
        assert!(
            init.arg("-C")
                .arg(&repo)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        if let Some(o) = origin {
            let mut add = amplihack_git::command();
            clean(&mut add);
            assert!(
                add.arg("-C")
                    .arg(&repo)
                    .args(["remote", "add", "origin", o])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let mut bash = std::process::Command::new("bash");
        clean(&mut bash);
        let out = bash
            .arg("-c")
            .arg("source \"$1\" && autodrive_state_key \"$2\" \"$3\"")
            .arg("parity")
            .arg(&script)
            .arg(&repo)
            .arg(ident)
            .output()
            .unwrap();
        assert!(out.status.success(), "bash failed for case {ident:?}");
        let want = String::from_utf8(out.stdout)
            .unwrap()
            .trim_end()
            .to_string();
        assert_eq!(autodrive::state_key(origin, ident), want, "case {ident:?}");
    }
}

#[test]
fn state_root_honours_only_an_absolute_state_dir() {
    let home = Path::new("/home/u");
    assert_eq!(
        autodrive::state_root(Some(std::ffi::OsStr::new("/srv/state")), home),
        PathBuf::from("/srv/state/auto-drive")
    );
    assert_eq!(
        autodrive::state_root(None, home),
        PathBuf::from("/home/u/.amplihack/state/auto-drive")
    );
    assert_eq!(
        autodrive::state_root(Some(std::ffi::OsStr::new("rel/state")), home),
        PathBuf::from("/home/u/.amplihack/state/auto-drive")
    );
}

fn state_dir() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("key");
    private_dir(&dir);
    (t, dir)
}

#[test]
fn autodrive_read_counts_rounds_and_phases() {
    let (_t, dir) = state_dir();
    private_file(
        &dir.join("phases.tsv"),
        "build\t2026-10-27T13:00:00Z\ncrusty-loop\t2026-10-27T13:30:00Z\n",
    );
    for f in [
        "crusty-round-1.json",
        "crusty-round-2.json",
        "crusty-round-2-before.json",
        "crusty-round-3.log",
        "crusty-round-2-health.json",
        "merge-ready-round-1.json",
    ] {
        private_file(&dir.join(f), "{}");
    }
    private_file(
        &dir.join("crusty-latest.json"),
        r#"{"concern_count":3,"concerns":["x"]}"#,
    );
    let s = autodrive::read(&dir).unwrap();
    assert_eq!(s.phases, ["build", "crusty-loop"]);
    assert_eq!(s.crusty_rounds, 2);
    assert_eq!(s.merge_ready_rounds, 1);
    assert_eq!(s.concerns, Some(3));
}

#[test]
fn autodrive_read_falls_back_to_record_rows() {
    let (_t, dir) = state_dir();
    private_file(&dir.join("merge-ready-records.tsv"), "1\tx\n\n2\ty\n3\tz\n");
    let s = autodrive::read(&dir).unwrap();
    assert_eq!(s.merge_ready_rounds, 3);
    assert_eq!(s.crusty_rounds, 0);
    assert!(s.phases.is_empty());
}

#[test]
fn autodrive_read_refuses_loose_or_symlinked_state() {
    let (t, dir) = state_dir();
    private_file(&dir.join("phases.tsv"), "build\n");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o775)).unwrap();
    assert!(autodrive::read(&dir).is_none(), "group-writable dir");

    let (_t2, dir2) = state_dir();
    let elsewhere = t.path().join("phases.tsv");
    private_file(&elsewhere, "merged\n");
    symlink(&elsewhere, dir2.join("phases.tsv")).unwrap();
    let s = autodrive::read(&dir2).unwrap_or_default();
    assert!(
        !s.phases.iter().any(|p| p == "merged"),
        "symlinked phases.tsv followed"
    );

    assert!(autodrive::read(&t.path().join("absent")).is_none());
}

#[test]
fn autodrive_read_survives_garbage_latest_json() {
    let (_t, dir) = state_dir();
    private_file(&dir.join("crusty-round-1.json"), "{}");
    private_file(&dir.join("crusty-latest.json"), r#"{"concern_count":-4}"#);
    assert_eq!(autodrive::read(&dir).unwrap().concerns, None);
}

// ================================================================== logins

fn api_profile(kind: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    match kind {
        "primary" => private_file(
            &t.path().join(".claude.json"),
            r#"{"primaryApiKey":"sk-SECRET"}"#,
        ),
        "empty-primary" => private_file(&t.path().join(".claude.json"), r#"{"primaryApiKey":""}"#),
        "helper" => private_file(
            &t.path().join("settings.json"),
            r#"{"apiKeyHelper":"/bin/key"}"#,
        ),
        "env" => private_file(
            &t.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-SECRET"}}"#,
        ),
        "oauth" => private_file(
            &t.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"a@b"}}"#,
        ),
        _ => {}
    }
    t
}

#[test]
fn api_key_profiles_are_detected_from_stored_settings() {
    for kind in ["primary", "helper", "env"] {
        assert!(
            logins::is_api_key_profile(api_profile(kind).path()),
            "{kind}"
        );
    }
    for kind in ["empty-primary", "oauth", "none"] {
        assert!(
            !logins::is_api_key_profile(api_profile(kind).path()),
            "{kind}"
        );
    }
}

#[test]
fn the_profile_name_plays_no_part_in_api_key_detection() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("api-key");
    private_dir(&dir);
    assert!(!logins::is_api_key_profile(&dir));
}

#[test]
fn readings_map_to_floored_clamped_percentages() {
    let LoginCell::Reading {
        session,
        week,
        state,
        reset,
    } = logins::cell_from(&report(UsageState::Ok, Some(72.9), Some(41.2)))
    else {
        panic!("ok with numbers is a reading");
    };
    assert_eq!((session, week), (Some(72), Some(41)));
    assert_eq!(state, "ok");
    assert!(
        reset.is_some_and(|r| r.contains("4pm")),
        "the next reset is shown"
    );
    let LoginCell::Reading { session, week, .. } = logins::cell_from(&report(
        UsageState::SessionExhausted,
        Some(150.0),
        Some(-3.0),
    )) else {
        panic!("exhausted with numbers is a reading");
    };
    assert_eq!((session, week), (Some(100), Some(0)));
    let LoginCell::Reading { session, week, .. } =
        logins::cell_from(&report(UsageState::Ok, Some(f64::NAN), Some(50.0)))
    else {
        panic!("one finite number is enough for a reading");
    };
    assert_eq!((session, week), (None, Some(50)));
}

#[test]
fn states_without_numbers_are_no_reading() {
    assert_eq!(
        logins::cell_from(&report(UsageState::NotLoggedIn, None, None)),
        LoginCell::NoReading("not_logged_in".into())
    );
    assert_eq!(
        logins::cell_from(&report(UsageState::Unparsed, Some(10.0), None)),
        LoginCell::NoReading("unparsed".into())
    );
    assert_eq!(
        logins::cell_from(&report(UsageState::Ok, None, None)),
        LoginCell::NoReading("ok".into())
    );
    assert_eq!(
        logins::cell_from(&report(UsageState::Ok, Some(f64::INFINITY), Some(f64::NAN))),
        LoginCell::NoReading("ok".into())
    );
}

fn stream(
    profiles: &[(&str, bool, bool)],
    cancelled: Arc<AtomicBool>,
) -> (mpsc::Sender<(String, UsageReport)>, UsageStream) {
    let (tx, rx) = mpsc::channel();
    let s = UsageStream {
        profiles: profiles
            .iter()
            .map(|(n, live, api)| ProfileInfo {
                name: n.to_string(),
                live: *live,
                api_key: *api,
            })
            .collect(),
        results: rx,
        cancel: Box::new(move || cancelled.store(true, Ordering::SeqCst)),
    };
    (tx, s)
}

#[test]
fn collect_keeps_store_order_and_marks_live_and_api_key() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, s) = stream(
        &[("a", false, false), ("b", true, false), ("k", false, true)],
        cancelled.clone(),
    );
    tx.send(("b".into(), report(UsageState::Ok, Some(10.0), Some(20.0))))
        .unwrap();
    tx.send(("a".into(), report(UsageState::NotLoggedIn, None, None)))
        .unwrap();
    let rows = logins::collect(s, Instant::now() + Duration::from_secs(5));
    let names: Vec<&str> = rows.iter().map(|r| r.profile.as_str()).collect();
    assert_eq!(names, ["a", "b", "k"]);
    assert!(rows[1].live && !rows[0].live);
    assert_eq!(rows[0].cell, LoginCell::NoReading("not_logged_in".into()));
    assert!(matches!(
        rows[1].cell,
        LoginCell::Reading {
            session: Some(10),
            week: Some(20),
            ..
        }
    ));
    assert_eq!(rows[2].cell, LoginCell::ApiKey);
}

#[test]
fn collect_returns_by_the_deadline_marks_timeouts_and_cancels() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, s) = stream(
        &[("fast", false, false), ("slow", false, false)],
        cancelled.clone(),
    );
    tx.send(("fast".into(), report(UsageState::Ok, Some(1.0), Some(2.0))))
        .unwrap();
    let begun = Instant::now();
    let rows = logins::collect(s, begun + Duration::from_millis(300));
    assert!(
        begun.elapsed() < Duration::from_millis(2300),
        "took {:?}",
        begun.elapsed()
    );
    assert!(matches!(rows[0].cell, LoginCell::Reading { .. }));
    assert_eq!(rows[1].cell, LoginCell::NoReading("timeout".into()));
    assert!(
        cancelled.load(Ordering::SeqCst),
        "pending probes are cancelled"
    );
    drop(tx);
}

#[test]
fn collect_ignores_results_for_unknown_profiles() {
    let (tx, s) = stream(&[("a", false, false)], Arc::default());
    tx.send(("intruder".into(), report(UsageState::Ok, Some(1.0), None)))
        .unwrap();
    tx.send(("a".into(), report(UsageState::Ok, Some(5.0), None)))
        .unwrap();
    let rows = logins::collect(s, Instant::now() + Duration::from_secs(2));
    assert_eq!(rows.len(), 1);
}

// ===================================================== run_with, end to end

const NO_SESSION: &str =
    "no amplihack session id in this environment; run with --all to list every run on the host";

#[test]
fn no_session_id_prints_the_note_then_logins_and_exits_zero() {
    let (_t, dir) = log_dir();
    add_run(
        &dir,
        "r",
        MetaFixture {
            parent_session_id: Some("S"),
            ..Default::default()
        },
    );
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Immediate),
    );
    let (code, out, _) = run_cmd(
        &args(false, None, false, Some(5)),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[]),
    );
    assert_eq!(code, 0);
    assert!(out.contains(NO_SESSION), "{out}");
    assert!(out.contains("LOGINS"), "{out}");
    assert!(
        !out.contains("auto-drive-to-merge"),
        "no runs without a scope: {out}"
    );
    assert_plain_and_narrow(&out);
}

#[test]
fn a_session_lists_its_runs_with_pr_state_and_logins() {
    let (t, dir) = log_dir();
    let cwd = t.path().to_str().unwrap().to_string();
    add_run(
        &dir,
        "amp-a",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1710),
            branch: Some("feat/issue-1710-x"),
            cwd: Some(&cwd),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "amp-b",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1712),
            cwd: Some(&cwd),
            ..Default::default()
        },
    );
    add_run(
        &dir,
        "amp-other",
        MetaFixture {
            parent_session_id: Some("T"),
            issue: Some(1800),
            cwd: Some(&cwd),
            ..Default::default()
        },
    );
    let live = FakeLive(vec![LiveSession {
        name: "amp-a".into(),
        created: None,
    }]);
    let prs = FakePrs {
        prs: Some(prs_fixture()),
        ..FakePrs::default()
    };
    let usage = FakeUsage::new(UsageMode::Immediate);
    let (code, out, _) = run_cmd(
        &args(false, None, false, Some(5)),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[("AMPLIHACK_SESSION_ID", "S")]),
    );
    assert_eq!(code, 0);
    let a = out
        .lines()
        .find(|l| l.contains("#1710"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        a.contains("auto-drive-to-merge #1710") && a.contains("#1715") && a.contains('▶'),
        "{a}"
    );
    assert!(a.contains("PR open"), "{a}");
    let b = out.lines().find(|l| l.contains("#1712")).unwrap();
    assert!(b.contains('⏸') && b.contains("building"), "{b}");
    assert!(
        !out.contains("#1800"),
        "another session's run leaked: {out}"
    );
    assert_eq!(prs.calls.load(Ordering::SeqCst), 1, "one gh call per repo");
    assert!(out.contains("main (live)"), "{out}");
    assert_plain_and_narrow(&out);
}

#[test]
fn a_failed_pr_lookup_says_pr_state_unknown_and_still_exits_zero() {
    let (t, dir) = log_dir();
    let cwd = t.path().to_str().unwrap().to_string();
    add_run(
        &dir,
        "amp-a",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1710),
            cwd: Some(&cwd),
            ..Default::default()
        },
    );
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Immediate),
    );
    let (code, out, _) = run_cmd(
        &args(false, None, true, None),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[("AMPLIHACK_SESSION_ID", "S")]),
    );
    assert_eq!(code, 0);
    assert!(out.contains("PR state unknown"), "{out}");
}

#[test]
fn no_logins_never_starts_a_probe() {
    let (_t, dir) = log_dir();
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Never),
    );
    let (code, out, _) = run_cmd(
        &args(true, None, true, None),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[]),
    );
    assert_eq!(code, 0);
    assert!(!usage.started.load(Ordering::SeqCst));
    assert!(!out.contains("LOGINS"), "{out}");
}

#[test]
fn a_probe_that_never_answers_ends_within_the_deadline() {
    let (_t, dir) = log_dir();
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Never),
    );
    let begun = Instant::now();
    let (code, out, _) = run_cmd(
        &args(true, None, false, Some(1)),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[]),
    );
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "took {:?}",
        begun.elapsed()
    );
    assert_eq!(code, 0);
    assert!(out.contains("no reading (timeout)"), "{out}");
    assert!(usage.cancelled.load(Ordering::SeqCst));
}

#[test]
fn a_missing_explicit_manifest_exits_two_and_cancels_probes() {
    let (t, dir) = log_dir();
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Never),
    );
    let (code, _, err) = run_cmd(
        &args(true, Some(t.path().join("missing.tsv")), false, Some(30)),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[]),
    );
    assert_eq!(code, 2);
    assert!(!err.is_empty());
    if usage.started.load(Ordering::SeqCst) {
        assert!(
            usage.cancelled.load(Ordering::SeqCst),
            "started probes must be cancelled on exit 2"
        );
    }
}

#[test]
fn a_symlinked_or_fifo_explicit_manifest_exits_two() {
    let (t, dir) = log_dir();
    let real = t.path().join("real.tsv");
    private_file(&real, "Label\t1\t-\n");
    let link = t.path().join("link.tsv");
    symlink(&real, &link).unwrap();
    let fifo = t.path().join("fifo.tsv");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Immediate),
    );
    for bad in [link, fifo] {
        let (code, _, _) = run_cmd(
            &args(true, Some(bad.clone()), true, None),
            &sources(&dir, &live, &prs, &usage),
            &env_of(&[]),
        );
        assert_eq!(code, 2, "{}", bad.display());
    }
}

#[test]
fn manifest_rows_merge_with_discovered_runs_and_queue_the_rest() {
    let (t, dir) = log_dir();
    add_run(
        &dir,
        "amp-a",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1710),
            ..Default::default()
        },
    );
    let m = t.path().join("ws.tsv");
    private_file(
        &m,
        "Workstream status skill\t#1710\t-\nDocs cleanup\t1712\t-\n",
    );
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs {
            prs: Some(Vec::new()),
            ..FakePrs::default()
        },
        FakeUsage::new(UsageMode::Immediate),
    );
    let (code, out, _) = run_cmd(
        &args(false, Some(m), true, None),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[("AMPLIHACK_SESSION_ID", "S")]),
    );
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    let a = lines
        .iter()
        .position(|l| l.contains("Workstream status skill"))
        .unwrap();
    let q = lines
        .iter()
        .position(|l| l.contains("Docs cleanup"))
        .unwrap();
    assert!(a < q, "manifest order kept");
    assert!(lines[a].contains("building"), "{}", lines[a]);
    assert!(
        lines[q].contains("queued") && lines[q].contains("  0%"),
        "{}",
        lines[q]
    );
    assert!(
        !out.contains("auto-drive-to-merge #1710"),
        "merged rows are not duplicated: {out}"
    );
}

#[test]
fn a_bad_env_deadline_warns_once_and_uses_the_default() {
    let (_t, dir) = log_dir();
    let (live, prs, usage) = (
        FakeLive::default(),
        FakePrs::default(),
        FakeUsage::new(UsageMode::Immediate),
    );
    for bad in ["0", "601", "abc", "-1"] {
        let pairs: &'static [(&str, &str)] = Box::leak(Box::new([(
            "AMPLIHACK_WORKSTREAMS_LOGINS_DEADLINE_SECS",
            bad,
        )]));
        let (code, _, err) = run_cmd(
            &args(true, None, false, None),
            &sources(&dir, &live, &prs, &usage),
            &env_of(pairs),
        );
        assert_eq!(code, 0);
        assert_eq!(
            err.matches("AMPLIHACK_WORKSTREAMS_LOGINS_DEADLINE_SECS")
                .count(),
            1,
            "{bad}: {err}"
        );
    }
}

#[test]
fn auto_drive_state_drives_the_stage_through_the_state_key() {
    let (t, dir) = log_dir();
    let cwd = t.path().to_str().unwrap().to_string();
    add_run(
        &dir,
        "amp-a",
        MetaFixture {
            parent_session_id: Some("S"),
            issue: Some(1710),
            pr: Some(1715),
            cwd: Some(&cwd),
            ..Default::default()
        },
    );
    let root = dir.with_file_name("state").join("auto-drive");
    let key = root.join(autodrive::state_key(
        Some("https://github.com/o/r.git"),
        "1715",
    ));
    private_dir(&key);
    private_file(&key.join("phases.tsv"), "build\t-\n");
    for n in 1..=2 {
        private_file(&key.join(format!("crusty-round-{n}.json")), "{}");
    }
    private_file(&key.join("crusty-latest.json"), r#"{"concern_count":3}"#);
    let live = FakeLive(vec![LiveSession {
        name: "amp-a".into(),
        created: None,
    }]);
    let prs = FakePrs {
        prs: Some(prs_fixture()),
        ..FakePrs::default()
    };
    let usage = FakeUsage::new(UsageMode::Immediate);
    let (_, out, _) = run_cmd(
        &args(false, None, true, None),
        &sources(&dir, &live, &prs, &usage),
        &env_of(&[("AMPLIHACK_SESSION_ID", "S")]),
    );
    let row = out
        .lines()
        .find(|l| l.contains("#1710"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        row.contains(" 48%") && row.contains("review round 2, 3 concerns"),
        "{row}"
    );
}

// ================================================================ security

#[test]
fn no_source_spawns_a_shell() {
    let dir = workspace_root().join("crates/amplihack-cli/src/commands/workstreams");
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == "tests.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for bad in [
            r#"Command::new("sh")"#,
            r#"Command::new("bash")"#,
            r#".arg("-c")"#,
        ] {
            assert!(!text.contains(bad), "{} contains {bad}", path.display());
        }
    }
}
