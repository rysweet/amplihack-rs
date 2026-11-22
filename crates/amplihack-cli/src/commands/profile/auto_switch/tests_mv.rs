//! `mv`: the login-moving primitives shared by auto-switch and `profile
//! rotate` (issue #1592). Files are checked by inode only; no login file is
//! ever read, so the fixtures are mode 0000 where the user is not root.
//!
//! Contract (declared in `auto_switch/mod.rs` as `#[cfg(all(test, unix))] mod
//! tests_mv;`):
//! - `mv::exchange_logins(by, a, b) -> Result<(), MoveError>`: the exchange
//!   `by` makes (`LoginExchange`, issue #2935), atomic when `by` is the
//!   host's; both paths must pass `movable_login`; `EXDEV`/`EINVAL`/`ENOSYS`
//!   map to `MoveError::Unsupported`; no copy fallback.
//! - `mv::HostExchange`: `renameat2(RENAME_EXCHANGE)` on Linux; not
//!   supported anywhere else. Only the tests marked Linux call it; every
//!   other test holds a `FakeExchange`, which records its calls.
//! - `mv::move_login(src, dst, show: &dyn Fn(&Path) -> String)`: no-replace
//!   rename; `EEXIST` maps to `MoveError::Exists`.
//! - `mv::take_refresh_lock_at(dir) -> io::Result<u64>` and
//!   `mv::release_refresh_lock_at(dir, ino) -> bool`.
//! - `mv::abort_point(step)`: a no-op in release builds.

use super::fsx;
use super::mv::{
    FakeExchange, HostExchange, LoginExchange, MoveError, RenameExchange, TempDirExchange,
    abort_point, exchange_for, exchange_logins, move_login, outside_temp_dir_line,
    release_refresh_lock_at, stand_in_requested, take_refresh_lock_at,
};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LOCK: &str = ".oauth_refresh.lock";

fn login(path: &Path) -> u64 {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "SENTINEL").unwrap();
    let mode = if fsx::euid() == 0 { 0o600 } else { 0o000 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    ino(path)
}

fn ino(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

fn show(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

struct Dirs {
    _tmp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    Dirs { _tmp: tmp, a, b }
}

#[cfg(target_os = "linux")]
#[test]
fn exchange_swaps_the_two_inodes() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    exchange_logins(&HostExchange, &pa, &pb).unwrap_or_else(|_| panic!("exchange failed"));
    assert_eq!(ino(&pa), ib);
    assert_eq!(ino(&pb), ia);
}

/// The stand-in that tests use on every system leaves what the host's
/// exchange leaves: each path holds the other file, by inode, and no spare
/// name is left behind.
#[test]
fn the_stand_in_exchange_swaps_the_two_inodes_and_leaves_nothing_else() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    exchange_logins(&RenameExchange, &pa, &pb).unwrap_or_else(|_| panic!("exchange failed"));
    assert_eq!((ino(&pa), ino(&pb)), (ib, ia));
    for dir in [&d.a, &d.b] {
        let names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [".credentials.json"], "{}", dir.display());
    }
}

/// The stand-in moves nothing when the second file is missing.
#[test]
fn the_stand_in_exchange_with_a_missing_side_moves_nothing() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let ia = login(&pa);
    assert!(RenameExchange.exchange(&pa, &pb).is_err());
    assert_eq!(ino(&pa), ia);
    assert_eq!(fs::read_dir(&d.a).unwrap().count(), 1);
    assert!(!fsx::exists(&pb));
}

/// Issue #2935, the first macOS refusal. The host's exchange exists only on
/// Linux. Everywhere else it reports that it is not supported, and asking
/// it anyway moves nothing and gives `Unsupported`, which the swap words as
/// "atomic exchange is not supported here".
#[test]
fn the_hosts_exchange_is_declined_on_every_system_but_linux() {
    assert_eq!(HostExchange.supported(), cfg!(target_os = "linux"));
    if HostExchange.supported() {
        return;
    }
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    assert!(matches!(
        exchange_logins(&HostExchange, &pa, &pb),
        Err(MoveError::Unsupported)
    ));
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
}

/// The same refusal on every system, from an exchange that is absent: the
/// call is made once, the answer is `Unsupported`, and nothing has moved.
#[test]
fn an_absent_exchange_is_unsupported_and_moves_nothing() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    let absent = FakeExchange::absent();
    assert!(!absent.supported());
    assert!(matches!(
        exchange_logins(absent.as_ref(), &pa, &pb),
        Err(MoveError::Unsupported)
    ));
    assert_eq!(absent.calls(), [(pa.clone(), pb.clone())]);
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
}

/// The stand-in is chosen by exactly `renames`, and only in a process that
/// also carries a test home variable that is not empty.
#[test]
fn the_stand_in_needs_its_exact_value_and_the_test_home_variable() {
    use std::ffi::OsStr;
    let cases: [(Option<&str>, Option<&str>, bool); 7] = [
        (Some("renames"), Some("1"), true),
        (Some("renames"), Some(""), false),
        (Some("renames"), None, false),
        (Some("Renames"), Some("1"), false),
        (Some("1"), Some("1"), false),
        (Some(""), Some("1"), false),
        (None, Some("1"), false),
    ];
    for (exchange, test_home, want) in cases {
        assert_eq!(
            stand_in_requested(exchange.map(OsStr::new), test_home.map(OsStr::new)),
            want,
            "{exchange:?} with {test_home:?}"
        );
    }
}

/// What the two variables select moves files inside the temp dir it was
/// given, as a test's logins are. `inside` stands for the binary's temp dir
/// and is a folder this test made, so nothing here reads the host's.
#[test]
fn the_selected_stand_in_swaps_two_logins_inside_the_temp_dir() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    let by = TempDirExchange::inside(d._tmp.path().to_path_buf());
    assert!(by.supported());
    assert!(exchange_logins(&by, &pa, &pb).is_ok());
    assert_eq!((ino(&pa), ino(&pb)), (ib, ia));
    assert_eq!(
        fs::read_dir(&d.a).unwrap().count(),
        1,
        "no spare name is left"
    );
}

/// The protection a debug build has when both variables are set by mistake:
/// a login outside the temp dir is refused and nothing moves, whichever side
/// it is on. A person's logins are under their home folder and the profile
/// root, never under the temp dir, so this is every real login.
#[test]
fn the_selected_stand_in_refuses_a_login_outside_the_temp_dir() {
    let inside = dirs();
    let outside = dirs();
    let real = outside.a.join(".credentials.json");
    let test = inside.a.join(".credentials.json");
    let (ireal, itest) = (login(&real), login(&test));
    let by = TempDirExchange::inside(inside._tmp.path().to_path_buf());

    for (a, b) in [(&real, &test), (&test, &real)] {
        let error = by.exchange(a, b).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        match exchange_logins(&by, a, b) {
            Err(MoveError::Other(why)) => assert_eq!(why, "exchange failed: permission denied"),
            _ => panic!("a login outside the temp dir must be refused, and not as unsupported"),
        }
        assert_eq!((ino(&real), ino(&test)), (ireal, itest), "nothing moved");
        for dir in [&outside.a, &inside.a] {
            assert_eq!(
                fs::read_dir(dir).unwrap().count(),
                1,
                "no spare name is left"
            );
        }
    }
}

/// A path that leaves the temp dir through `..` or through a link is outside
/// it: the test is on where the file is, not on how its name begins.
#[test]
fn the_selected_stand_in_is_not_fooled_by_dots_or_a_link() {
    let inside = dirs();
    let outside = dirs();
    let real = outside.a.join(".credentials.json");
    let test = inside.a.join(".credentials.json");
    let (ireal, itest) = (login(&real), login(&test));
    let by = TempDirExchange::inside(inside._tmp.path().to_path_buf());

    let link = inside.b.join("link");
    std::os::unix::fs::symlink(&outside.a, &link).unwrap();
    let through_link = link.join(".credentials.json");
    let through_dots = inside
        .a
        .join("..")
        .join("..")
        .join(outside._tmp.path().file_name().unwrap())
        .join("a")
        .join(".credentials.json");
    assert_eq!(
        ino(&through_dots),
        ireal,
        "the dotted path does name the outside login"
    );

    for disguised in [&through_link, &through_dots] {
        assert!(
            by.exchange(disguised, &test).is_err(),
            "{}",
            disguised.display()
        );
        assert_eq!((ino(&real), ino(&test)), (ireal, itest), "nothing moved");
    }
}

/// What a command is handed for the two variables' values. Both set: the
/// stand-in, which swaps a pair inside the temp dir and refuses a login
/// outside it. Either one missing: the host's exchange, which is present on
/// Linux alone. So the variables never give a command plain renames that
/// reach outside the temp dir.
#[test]
fn a_command_gets_the_confined_stand_in_or_the_hosts_exchange() {
    use std::ffi::OsStr;
    let inside = dirs();
    let outside = dirs();
    let temp_dir = inside._tmp.path().to_path_buf();
    let (pa, pb) = (
        inside.a.join(".credentials.json"),
        inside.b.join(".credentials.json"),
    );
    let real = outside.a.join(".credentials.json");
    let (ia, ib, ireal) = (login(&pa), login(&pb), login(&real));

    let chosen = exchange_for(
        Some(OsStr::new("renames")),
        Some(OsStr::new("1")),
        temp_dir.clone(),
    );
    assert!(chosen.supported());
    assert!(chosen.exchange(&real, &pa).is_err());
    assert_eq!((ino(&real), ino(&pa)), (ireal, ia), "nothing moved");
    chosen.exchange(&pa, &pb).unwrap();
    assert_eq!((ino(&pa), ino(&pb)), (ib, ia));

    for (exchange, test_home) in [(Some("renames"), None), (None, Some("1")), (None, None)] {
        let host = exchange_for(
            exchange.map(OsStr::new),
            test_home.map(OsStr::new),
            temp_dir.clone(),
        );
        assert_eq!(
            host.supported(),
            cfg!(target_os = "linux"),
            "{exchange:?} with {test_home:?} is the host's exchange"
        );
    }
}

/// The refusal says which variable, which folder and which file, and how to
/// get the system's own exchange.
#[test]
fn the_refusal_line_names_the_variable_the_temp_dir_and_the_file() {
    let line = outside_temp_dir_line(
        Path::new("/home/u/.claude/.credentials.json"),
        Path::new("/tmp/t"),
    );
    assert!(
        line.starts_with("amplihack: AMPLIHACK_TEST_LOGIN_EXCHANGE "),
        "{line}"
    );
    assert!(line.contains("inside the temp dir (/tmp/t) only"), "{line}");
    assert!(
        line.contains("/home/u/.claude/.credentials.json is outside it"),
        "{line}"
    );
    assert!(line.contains("nothing was moved"), "{line}");
    assert!(line.contains("Unset the variable"), "{line}");
}

#[test]
fn exchange_refuses_a_missing_side_and_moves_nothing() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let ia = login(&pa);
    let by = FakeExchange::working();
    assert!(exchange_logins(by.as_ref(), &pa, &pb).is_err());
    assert!(
        by.calls().is_empty(),
        "refused before the exchange is asked"
    );
    assert_eq!(ino(&pa), ia);
    assert!(!fsx::exists(&pb));
}

#[test]
fn exchange_refuses_a_hard_linked_or_symlinked_login() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let ia = login(&pa);
    let ib = login(&pb);
    let by = FakeExchange::working();
    fs::hard_link(&pb, d.b.join("extra-link")).unwrap();
    assert!(exchange_logins(by.as_ref(), &pa, &pb).is_err(), "two links");
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));

    fs::remove_file(&pb).unwrap();
    std::os::unix::fs::symlink(d.b.join("extra-link"), &pb).unwrap();
    assert!(exchange_logins(by.as_ref(), &pa, &pb).is_err(), "symlink");
    assert!(
        by.calls().is_empty(),
        "refused before the exchange is asked"
    );
    assert_eq!(ino(&pa), ia);
    assert!(fs::symlink_metadata(&pb).unwrap().file_type().is_symlink());
}

/// A login other users could read stays put: moving it into a
/// group-readable `~/.claude` would expose it.
#[test]
fn a_login_readable_by_others_is_not_moved() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    fs::set_permissions(&pb, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fsx::movable_login(&pb));
    let by = FakeExchange::working();
    assert!(exchange_logins(by.as_ref(), &pa, &pb).is_err(), "exchange");
    assert!(
        by.calls().is_empty(),
        "refused before the exchange is asked"
    );
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
    fs::remove_file(&pa).unwrap();
    assert!(move_login(&pb, &pa, &show).is_err(), "move");
    assert_eq!(ino(&pb), ib);
    assert!(!fsx::exists(&pa));
}

/// A cross-device exchange is `Unsupported`, and nothing is copied. Needs a
/// second filesystem; `/dev/shm` usually is one.
#[cfg(target_os = "linux")]
#[test]
fn exchange_across_devices_is_unsupported_without_a_copy() {
    let d = dirs();
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    if fs::metadata(other.path()).unwrap().dev() == fs::metadata(&d.a).unwrap().dev() {
        return;
    }
    let (pa, pb) = (
        d.a.join(".credentials.json"),
        other.path().join(".credentials.json"),
    );
    let (ia, ib) = (login(&pa), login(&pb));
    assert!(matches!(
        exchange_logins(&HostExchange, &pa, &pb),
        Err(MoveError::Unsupported)
    ));
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
}

#[test]
fn move_login_never_replaces() {
    let d = dirs();
    let (src, dst) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (is, id) = (login(&src), login(&dst));
    assert!(matches!(
        move_login(&src, &dst, &show),
        Err(MoveError::Exists)
    ));
    assert_eq!((ino(&src), ino(&dst)), (is, id));

    fs::rename(&dst, d.b.join("aside")).unwrap();
    assert!(move_login(&src, &dst, &show).is_ok());
    assert_eq!(ino(&dst), is);
    assert!(!fsx::exists(&src));
}

#[test]
fn move_login_error_uses_the_display_closure() {
    let d = dirs();
    let src = d.a.join(".credentials.json");
    let Err(MoveError::Other(msg)) = move_login(&src, &d.b.join("x"), &|_| "SHOWN".into()) else {
        panic!("a missing source is an error");
    };
    assert!(msg.contains("SHOWN"), "{msg}");
    assert!(!msg.contains(&*d.a.to_string_lossy()), "{msg}");
}

#[test]
fn refresh_lock_is_taken_and_released_by_inode() {
    let d = dirs();
    let ino_taken = take_refresh_lock_at(&d.a).expect("free lock is taken");
    assert_eq!(ino(&d.a.join(LOCK)), ino_taken);
    assert!(take_refresh_lock_at(&d.a).is_err(), "already held");
    assert!(release_refresh_lock_at(&d.a, ino_taken));
    assert!(!fsx::exists(&d.a.join(LOCK)));
}

#[test]
fn a_foreign_refresh_lock_is_never_broken() {
    let d = dirs();
    fs::create_dir(d.a.join(LOCK)).unwrap();
    let foreign = ino(&d.a.join(LOCK));
    assert!(take_refresh_lock_at(&d.a).is_err());
    assert!(!release_refresh_lock_at(&d.a, foreign + 1));
    assert_eq!(ino(&d.a.join(LOCK)), foreign, "left in place");
}

#[test]
fn release_ignores_a_regular_file_with_the_lock_name() {
    let d = dirs();
    let i = login(&d.a.join(LOCK));
    assert!(!release_refresh_lock_at(&d.a, i));
    assert!(fsx::exists(&d.a.join(LOCK)));
}

#[cfg(not(debug_assertions))]
#[test]
fn abort_point_is_a_no_op_in_release_builds() {
    // SAFETY: test-only; no other thread reads this variable.
    unsafe { std::env::set_var("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER", "rotate-S1") };
    abort_point("rotate-S1");
    unsafe { std::env::remove_var("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER") };
}

#[cfg(debug_assertions)]
#[test]
fn abort_point_ignores_other_steps() {
    abort_point("rotate-never-set");
}
