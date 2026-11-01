//! Issue #1803: the `--supervised[=UNIT]` grammar of `rotate watch`.
//!
//! Contract: `RotateAction::Watch(WatchArgs)`, where `WatchArgs` flattens
//! `WatchFlags` as `flags` and adds `supervised: Option<Option<String>>`:
//! `None` without the flag, `Some(None)` bare, `Some(Some(unit))` with `=`.
//! `Start` keeps `WatchFlags` and has no such flag.

use super::{RotateAction, RotateArgs, WatchArgs};
use clap::Parser;

#[derive(Debug, Parser)]
struct Cli {
    #[command(flatten)]
    rotate: RotateArgs,
}

fn parse(args: &[&str]) -> Result<RotateArgs, clap::Error> {
    let argv = std::iter::once("rotate").chain(args.iter().copied());
    Cli::try_parse_from(argv).map(|c| c.rotate)
}

fn watch(args: &[&str]) -> WatchArgs {
    match parse(args).unwrap().action {
        Some(RotateAction::Watch(w)) => w,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_flag_is_optional() {
    assert_eq!(watch(&["watch"]).supervised, None);
}

#[test]
fn a_bare_flag_has_no_unit() {
    assert_eq!(watch(&["watch", "--supervised"]).supervised, Some(None));
}

#[test]
fn a_unit_is_given_with_equals() {
    assert_eq!(
        watch(&["watch", "--supervised=rot.service"]).supervised,
        Some(Some("rot.service".to_string()))
    );
}

#[test]
fn the_flag_does_not_take_the_next_token() {
    assert!(parse(&["watch", "--supervised", "rot.service"]).is_err());
    assert!(parse(&["watch", "--supervised", "30"]).is_err());
    let w = watch(&["watch", "--supervised", "--interval", "30"]);
    assert_eq!(w.supervised, Some(None));
    assert_eq!(w.flags.interval.as_deref(), Some("30"));
}

#[test]
fn the_flag_conflicts_with_spawned() {
    let e = parse(&["watch", "--supervised", "--spawned"]).unwrap_err();
    assert_eq!(e.kind(), clap::error::ErrorKind::ArgumentConflict);
    let e = parse(&["watch", "--spawned", "--supervised=a.service"]).unwrap_err();
    assert_eq!(e.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn start_has_no_such_flag() {
    let e = parse(&["start", "--supervised"]).unwrap_err();
    assert_eq!(e.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn spawned_and_the_tuning_flags_still_parse() {
    let w = watch(&["watch", "--spawned", "--session-at", "70"]);
    assert!(w.flags.spawned);
    assert_eq!(w.flags.tune.session_at.as_deref(), Some("70"));
}
