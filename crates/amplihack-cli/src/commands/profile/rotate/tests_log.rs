//! `rotate::log`: line format, size rotation and the refusal to write to a
//! log that is not private (issue #1592).
//!
//! Contract:
//! - `log::line(now: i64, msg: &str) -> String`: `YYYY-MM-DDTHH:MM:SSZ msg`
//!   with control characters removed from `msg`.
//! - `log::MAX_LOG: u64` (1 MiB).
//! - `RotateLog::open(log: &Path, rotated: &Path) -> io::Result<RotateLog>`
//!   and `RotateLog::write(&mut self, now: i64, msg: &str) -> io::Result<()>`.
//! - `log::msg::{swap, cycle, alert_no_capacity}` build the documented lines.

use super::decide::Reading;
use super::log::{MAX_LOG, RotateLog, Shown, line, msg};
use crate::commands::profile::name::ProfileName;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// 2025-10-09T08:53:20Z
const T: i64 = 1_760_000_000;

struct Paths {
    _tmp: tempfile::TempDir,
    log: PathBuf,
    log1: PathBuf,
}

fn paths() -> Paths {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("rotate");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    Paths {
        log: dir.join("rotate.log"),
        log1: dir.join("rotate.log.1"),
        _tmp: tmp,
    }
}

fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

#[test]
fn a_line_is_a_utc_time_then_the_message() {
    assert_eq!(
        line(T, "idle: auto-switch is off"),
        "2025-10-09T08:53:20Z idle: auto-switch is off"
    );
}

#[test]
fn control_characters_are_removed_from_messages() {
    let l = line(T, "skip: a\x1b[31m\nb");
    assert!(!l.contains('\x1b') && !l.contains('\n'), "{l:?}");
}

#[test]
fn message_grammar() {
    let m = |session: f64, week: f64| Shown::from_reading(Reading::Measured { session, week });
    assert_eq!(
        msg::swap(
            &name("work"),
            &name("spare"),
            &m(90.0, 40.0),
            &m(10.0, 20.0),
            super::decide::Reason::Threshold
        ),
        "swap work->spare live s=90% w=40% target s=10% w=20% reason=threshold"
    );
    assert_eq!(
        msg::cycle(
            Some(&name("work")),
            &m(90.0, 40.0),
            &[
                (name("b"), Shown::from_reading(Reading::Failed)),
                (name("a"), m(1.0, 2.0)),
            ],
            None
        ),
        "cycle live=work work:s=90%,w=40% a:s=1%,w=2% b:failed"
    );
    assert_eq!(
        msg::cycle(None, &Shown::from_reading(Reading::Failed), &[], None),
        "cycle live=- -:failed"
    );
    // The API-key profile shows as per-token, live or not (#1700). An
    // exhausted reading with no windows read shows `w=?` (#1844), never a
    // made-up `w=100%`.
    let key = name("key");
    let platform = m(0.0, 0.0);
    let exhausted = Shown::from_reading(Reading::Exhausted);
    assert_eq!(
        msg::cycle(
            Some(&name("work")),
            &exhausted,
            &[(key.clone(), platform.clone())],
            Some(&key)
        ),
        "cycle live=work work:s=100%,w=? key:per-token"
    );
    assert_eq!(
        msg::cycle(
            Some(&key),
            &platform,
            &[(name("work"), exhausted)],
            Some(&key)
        ),
        "cycle live=key key:per-token work:s=100%,w=?"
    );
    assert_eq!(
        msg::alert_no_capacity(
            &name("work"),
            &Reading::Measured {
                session: 99.0,
                week: 50.0
            }
        ),
        "ALERT: no profile has capacity live=work s=99% w=50%"
    );
}

#[test]
fn the_log_is_created_private_and_appended() {
    let p = paths();
    let mut l = RotateLog::open(&p.log, &p.log1).unwrap();
    l.write(T, "one").unwrap();
    l.write(T + 1, "two").unwrap();
    drop(l);
    let mode = fs::metadata(&p.log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        fs::read_to_string(&p.log).unwrap(),
        "2025-10-09T08:53:20Z one\n2025-10-09T08:53:21Z two\n"
    );
}

#[test]
fn a_log_over_one_mebibyte_is_rotated_before_the_write() {
    let p = paths();
    fs::write(&p.log1, "old rotated log\n").unwrap();
    fs::write(&p.log, vec![b'x'; MAX_LOG as usize + 1]).unwrap();
    fs::set_permissions(&p.log, fs::Permissions::from_mode(0o600)).unwrap();
    let mut l = RotateLog::open(&p.log, &p.log1).unwrap();
    l.write(T, "fresh").unwrap();
    assert_eq!(
        fs::metadata(&p.log1).unwrap().len(),
        MAX_LOG + 1,
        "replaced"
    );
    assert_eq!(
        fs::read_to_string(&p.log).unwrap(),
        "2025-10-09T08:53:20Z fresh\n"
    );
}

#[test]
fn a_log_at_exactly_one_mebibyte_is_not_rotated() {
    let p = paths();
    fs::write(&p.log, vec![b'x'; MAX_LOG as usize]).unwrap();
    fs::set_permissions(&p.log, fs::Permissions::from_mode(0o600)).unwrap();
    RotateLog::open(&p.log, &p.log1)
        .unwrap()
        .write(T, "m")
        .unwrap();
    assert!(!p.log1.exists());
}

#[test]
fn a_symlinked_log_is_refused() {
    let p = paths();
    let target = p.log.with_file_name("elsewhere");
    fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &p.log).unwrap();
    let opened = RotateLog::open(&p.log, &p.log1).and_then(|mut l| l.write(T, "x"));
    assert!(opened.is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "");
}

#[test]
fn a_log_readable_by_others_is_refused_and_its_mode_kept() {
    let p = paths();
    fs::write(&p.log, "").unwrap();
    fs::set_permissions(&p.log, fs::Permissions::from_mode(0o644)).unwrap();
    let opened = RotateLog::open(&p.log, &p.log1).and_then(|mut l| l.write(T, "x"));
    assert!(opened.is_err());
    assert_eq!(fs::read_to_string(&p.log).unwrap(), "");
    let mode = fs::metadata(&p.log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "the mode is not changed");
}

#[test]
fn a_fifo_log_is_refused_without_blocking() {
    let p = paths();
    let c = std::ffi::CString::new(p.log.to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(RotateLog::open(&p.log, &p.log1).is_err());
}

// --- Issue #1693: shell quoting and the lost-login line ---------------------
//
// Contract: `log::sh_quote(&Path) -> Option<String>`: bare when every byte
// is in `A-Za-z0-9/._+-`, else single-quoted with each `'` as `'\''`;
// `None` for a path with a control or bidirectional-override character, or
// one that is not UTF-8. `msg::lost_login(&ProfileName) -> String`.

mod handfix_text {
    use super::super::log::{msg, sh_quote};
    use super::name;
    use std::path::Path;

    fn q(p: &str) -> Option<String> {
        sh_quote(Path::new(p))
    }

    #[test]
    fn safe_paths_are_bare() {
        assert_eq!(
            q("/home/you/.claude-profiles/a_b+c.d-1"),
            Some("/home/you/.claude-profiles/a_b+c.d-1".into())
        );
    }

    #[test]
    fn other_paths_are_single_quoted() {
        assert_eq!(q("/home/a b"), Some("'/home/a b'".into()));
        assert_eq!(q("/home/it's"), Some(r"'/home/it'\''s'".into()));
        assert_eq!(q("/home/$HOME`x`;y"), Some("'/home/$HOME`x`;y'".into()));
        assert_eq!(q("~/x"), Some("'~/x'".into()), "never a bare ~");
        assert_eq!(q("/home/é"), Some("'/home/é'".into()));
    }

    #[test]
    fn unprintable_paths_have_no_command() {
        for p in [
            "/a\nb",
            "/a\tb",
            "/a\u{1b}b",
            "/a\u{202e}b",
            "/a\u{2069}b",
            "/a\u{7f}b",
        ] {
            assert_eq!(q(p), None, "{p:?}");
        }
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::OsStr::from_bytes(b"/a\xffb");
        assert_eq!(sh_quote(Path::new(raw)), None);
    }

    #[test]
    fn the_lost_login_line_names_the_profile_and_the_command() {
        assert_eq!(
            msg::lost_login(&name("ryan-ryansweet")),
            "ALERT: profile ryan-ryansweet has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login ryan-ryansweet"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1844: the log shows the windows read, not a fixed `w=100%`.
//
// Contract:
// - `log::Shown { reading, session: Option<f64>, week: Option<f64>,
//   still_running: bool }` derives `Debug`, `Clone` and `PartialEq`.
// - `Shown::of(&Probed)` takes the reading and the session and weekly
//   percentages from `Probed::windows`, and `still_running` from
//   `Probed::still_running`.
// - `Shown::from_reading(Reading)` takes the percentages from a
//   `Measured` reading and has no windows otherwise.
// - `msg::usage(&Shown, sep)`: `s=<n>%<sep>w=<n>%`, with `s=100%` at least
//   for an exhausted reading and `w=?` when no weekly window is known;
//   `failed` for a failed reading; `unknown, still running` when
//   `still_running` is set, whatever the reading.
// - `msg::swap` and `msg::cycle` take `&Shown` and use the same format.
// ---------------------------------------------------------------------------

use super::decide::{Probed, Win, Windows};

fn probed(reading: Reading, session: Option<f64>, week: Option<f64>) -> Probed {
    let win = |used: f64| Win {
        used,
        resets_at: Some(T + 3600),
    };
    Probed {
        reading,
        windows: Windows {
            session: session.map(win),
            week: week.map(win),
        },
        ..Probed::failed()
    }
}

fn exhausted(session: Option<f64>, week: Option<f64>) -> Shown {
    let mut p = probed(Reading::Exhausted, session, week);
    p.unknown = false;
    Shown::of(&p)
}

#[test]
fn an_exhausted_session_shows_the_weekly_figure_read() {
    assert_eq!(
        msg::usage(&exhausted(Some(100.0), Some(55.0)), ","),
        "s=100%,w=55%"
    );
    assert_eq!(
        msg::usage(&exhausted(Some(100.0), Some(55.0)), " "),
        "s=100% w=55%"
    );
}

#[test]
fn an_exhausted_login_shows_at_least_100_for_the_session() {
    // A session window below 100% on an exhausted reading (the week ran
    // out) still shows `s=100%`; one above 100% shows as read.
    assert_eq!(
        msg::usage(&exhausted(Some(40.0), Some(100.0)), ","),
        "s=100%,w=100%"
    );
    assert_eq!(
        msg::usage(&exhausted(Some(130.0), Some(55.0)), ","),
        "s=130%,w=55%"
    );
    assert_eq!(
        msg::usage(&exhausted(None, Some(55.0)), ","),
        "s=100%,w=55%"
    );
}

#[test]
fn no_weekly_window_shows_a_question_mark() {
    assert_eq!(msg::usage(&exhausted(Some(100.0), None), ","), "s=100%,w=?");
    let mut p = probed(
        Reading::Measured {
            session: 20.0,
            week: 0.0,
        },
        Some(20.0),
        None,
    );
    p.unknown = false;
    assert_eq!(msg::usage(&Shown::of(&p), ","), "s=20%,w=?");
}

#[test]
fn a_measured_reading_shows_its_windows_rounded_down() {
    let mut p = probed(
        Reading::Measured {
            session: 41.9,
            week: 30.2,
        },
        Some(41.9),
        Some(30.2),
    );
    p.unknown = false;
    assert_eq!(msg::usage(&Shown::of(&p), ","), "s=41%,w=30%");
}

#[test]
fn a_read_still_running_shows_unknown_still_running() {
    let mut p = Probed::failed();
    p.still_running = true;
    let s = Shown::of(&p);
    assert!(s.still_running);
    assert_eq!(msg::usage(&s, ","), "unknown, still running");
    assert_eq!(
        msg::cycle(Some(&name("work")), &s, &[], None),
        "cycle live=work work:unknown, still running"
    );
}

#[test]
fn a_failed_read_still_shows_failed() {
    let s = Shown::of(&Probed::failed());
    assert!(!s.still_running);
    assert_eq!(msg::usage(&s, ","), "failed");
}

#[test]
fn probed_is_not_still_running_by_default() {
    assert!(!Probed::failed().still_running);
    assert!(!Probed::platform().still_running);
}

#[test]
fn swap_and_cycle_lines_show_the_weekly_figure_of_an_exhausted_login() {
    let live = exhausted(Some(100.0), Some(72.0));
    let mut t = probed(
        Reading::Measured {
            session: 0.0,
            week: 43.0,
        },
        Some(0.0),
        Some(43.0),
    );
    t.unknown = false;
    let target = Shown::of(&t);
    assert_eq!(
        msg::swap(
            &name("work"),
            &name("spare"),
            &live,
            &target,
            super::decide::Reason::Threshold
        ),
        "swap work->spare live s=100% w=72% target s=0% w=43% reason=threshold"
    );
    assert_eq!(
        msg::cycle(
            Some(&name("work")),
            &live,
            &[(name("team"), exhausted(Some(100.0), Some(55.0)))],
            None
        ),
        "cycle live=work work:s=100%,w=72% team:s=100%,w=55%"
    );
}

#[test]
fn shown_from_reading_keeps_measured_figures() {
    let s = Shown::from_reading(Reading::Measured {
        session: 12.0,
        week: 34.0,
    });
    assert_eq!(s.session, Some(12.0));
    assert_eq!(s.week, Some(34.0));
    assert!(!s.still_running);
    assert_eq!(msg::usage(&s, ","), "s=12%,w=34%");
}
