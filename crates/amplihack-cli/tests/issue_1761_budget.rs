//! Issue #1761: all usage-limit waits of one recipe run share a time
//! budget, kept in `$HOME/.amplihack/rerun/wait-budget-<run id>`. Only a
//! wait with a rotator running draws on it, so every step here runs in
//! `$HOME/.claude` with a fake rotator whose last cycle left no capacity
//! (`held`): no wait ends early on the rotator.
//!
//! - Without a rotator a step keeps its single wait to the reset, however
//!   much of the run's budget other steps spent, and writes no budget
//!   warning; its seconds still go to the record, so a later step with a
//!   rotator sees them.
//! - AC3: a step waits until the budget is spent and ends with
//!   `end reason=budget spent` and a plain line naming the setting; the
//!   next step of the same run writes `skipped reason=budget_spent`; a step
//!   of another run starts afresh.
//! - Without a run id, or with an invalid one, a step keeps its own budget,
//!   writes one fixed warning that never quotes the value, and creates no
//!   record.
//! - SR-10: a symlink planted at the record's path is refused and never
//!   followed; the step falls back with one warning and still waits.
//! - SR-13: a limit refused before the budget check touches no record.
//! - A budget of 0 refuses every wait; the record still exists.
//!
//! See `issue_1618_fixture` and `issue_1761_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_TEXT, describe, events, raw_events, text,
};
use crate::issue_1761_fixture::{
    BUDGET_VAR, FakeRotator, LIVE, NOT_SET, NOT_VALID, RUN_ID_VAR, UNUSABLE, WAITS_VAR,
    assert_lines, budget_record, budget_records, budget_warnings, home_claude, line_after,
    record_lines, run_budget_note, run_id, step_budget_note, waited, warnings, write_ledger,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Starts the fake rotator, its last cycle `held`. Keep it for the test.
fn rotator(f: &Fixture) -> FakeRotator {
    let r = FakeRotator::start(f);
    write_ledger(f, LIVE, "held");
    r
}

/// Step `name` of recipe run `id` (`None`: no run id), with `budget`
/// seconds of budget, in the fixture's own config directory: no rotator
/// counts for it.
fn plain_step(f: &Fixture, name: &str, id: Option<&str>, budget: &str) -> Command {
    let mut c = f.step();
    c.env("STEP", name).env(BUDGET_VAR, budget);
    if let Some(id) = id {
        c.env(RUN_ID_VAR, id);
    }
    c
}

/// [`plain_step`] in `$HOME/.claude`, where [`rotator`] counts.
fn step(f: &Fixture, name: &str, id: Option<&str>, budget: &str) -> Command {
    let mut c = plain_step(f, name, id, budget);
    home_claude(f, &mut c);
    c
}

#[test]
fn ac3_a_spent_budget_ends_the_run_with_the_budget_reason() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let id = run_id();

    // A: the reset is 30 minutes away, the budget 10.
    let a = f.run(&mut step(&f, "a", Some(&id), "600"));
    assert_eq!(a.status.code(), Some(1), "{}", describe(&a));
    assert_eq!(f.count("a"), 1, "no retry once the budget is spent");
    let ev = raw_events(&a);
    assert_lines(
        &ev,
        &[
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:10:00 -07:00 bound=budget step=\\d+ attempt=1 rotator=running",
            "end reason=budget spent waited=\\d+s",
        ],
    );
    assert_eq!(
        line_after(&a, "end reason=budget spent"),
        Some(run_budget_note(600))
    );
    assert!(
        text(&a.stderr).contains(LIMIT_TEXT),
        "the runner still sees the limit"
    );
    assert!(budget_warnings(&a).is_empty(), "{:?}", budget_warnings(&a));
    let record = budget_record(&f, &id);
    assert_eq!(record_lines(&record), waited(&ev, "end "));
    let meta = fs::metadata(&record).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    // B: the same run; nothing is left.
    let b = f.run(&mut step(&f, "b", Some(&id), "600"));
    assert_eq!(b.status.code(), Some(1), "{}", describe(&b));
    assert_eq!(f.count("b"), 1);
    assert_lines(&raw_events(&b), &["skipped reason=budget_spent step=\\d+"]);
    assert_eq!(
        line_after(&b, "skipped reason=budget_spent"),
        Some(run_budget_note(600))
    );
    assert_eq!(
        record_lines(&record),
        waited(&ev, "end "),
        "a refused limit adds nothing"
    );

    // C: another run has its own budget.
    let other = run_id();
    let c = f.run(&mut step(&f, "c", Some(&other), "600"));
    assert!(
        raw_events(&c).iter().any(|l| l.starts_with("start ")),
        "{}",
        describe(&c)
    );
    assert!(budget_record(&f, &other).exists());
    f.assert_isolated("a");
}

#[test]
fn the_budget_left_bounds_a_later_wait_of_the_same_run() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let id = run_id();
    let a = f.run(&mut step(&f, "a", Some(&id), "2400"));
    assert!(a.status.success(), "{}", describe(&a));
    let spent: u64 = record_lines(&budget_record(&f, &id)).iter().sum();
    assert!(spent > 0 && spent < 2400, "{spent}");
    // The next step's wait would end at 08:32; the 2400 s budget ends it
    // sooner, named `budget`.
    let b = f.run(&mut step(&f, "b", Some(&id), "2400"));
    assert_eq!(b.status.code(), Some(1), "{}", describe(&b));
    let ev = raw_events(&b);
    assert!(ev[0].contains(" bound=budget step="), "{ev:?}");
    assert_eq!(
        ev[1],
        format!("end reason=budget spent waited={}s", 2400 - spent)
    );
    let total: u64 = record_lines(&budget_record(&f, &id)).iter().sum();
    assert_eq!(total, 2400);
}

#[test]
fn without_a_run_id_the_step_keeps_its_own_budget_and_warns_once() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let out = f.run(&mut step(&f, "0", None, "600"));
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(budget_warnings(&out), vec![NOT_SET.to_string()]);
    assert_eq!(
        events(&out),
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:10:00 -07:00 bound=budget",
            "end reason=budget spent",
        ]
    );
    assert_eq!(
        line_after(&out, "end reason=budget spent"),
        Some(step_budget_note(600))
    );
    assert!(budget_records(&f).is_empty(), "{:?}", budget_records(&f));
}

#[test]
fn a_step_outside_a_recipe_run_still_waits_once_with_one_warning() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let mut c = f.step();
    home_claude(&f, &mut c);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(budget_warnings(&out), vec![NOT_SET.to_string()]);
    assert!(budget_records(&f).is_empty());
}

#[test]
fn without_a_rotator_or_a_run_id_the_single_wait_writes_no_budget_warning() {
    let f = Fixture::new();
    let out = f.run(&mut f.step());
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, one retry");
    assert!(
        budget_warnings(&out).is_empty(),
        "the budget does not apply: {:?}",
        budget_warnings(&out)
    );
    assert!(budget_records(&f).is_empty());
}

/// #1761 keeps the #1618 single wait while no rotator runs: two such steps
/// of one run, whose waits add up to more than the budget, each wait the
/// whole way to the reset. Their seconds are recorded, so a third step,
/// with a rotator, finds the budget spent.
#[test]
fn without_a_rotator_each_step_keeps_its_whole_single_wait_whatever_the_run_spent() {
    let f = Fixture::new();
    let id = run_id();
    let full = "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
                until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=1 rotator=none";
    let mut spent = Vec::new();
    for name in ["a", "b"] {
        let out = f.run(&mut plain_step(&f, name, Some(&id), "2400"));
        assert!(out.status.success(), "{name}: {}", describe(&out));
        assert_eq!(f.count(name), 2, "{name}: one wait, one retry");
        let ev = raw_events(&out);
        assert_lines(
            &ev,
            &[
                full,
                "end reason=reset reached waited=\\d+s",
                "resume attempt=2 step=\\d+ waited=\\d+s login=unknown",
            ],
        );
        assert!(
            budget_warnings(&out).is_empty(),
            "{name}: {:?}",
            budget_warnings(&out)
        );
        spent.extend(waited(&ev, "end "));
    }
    let total: u64 = spent.iter().sum();
    assert!(total > 2400, "the two waits ({spent:?}) exceed the budget");
    assert_eq!(record_lines(&budget_record(&f, &id)), spent);

    let _r = rotator(&f);
    let c = f.run(&mut step(&f, "c", Some(&id), "2400"));
    assert_eq!(c.status.code(), Some(1), "{}", describe(&c));
    assert_eq!(f.count("c"), 1);
    assert_lines(&raw_events(&c), &["skipped reason=budget_spent step=\\d+"]);
}

#[test]
fn an_invalid_run_id_is_never_used_or_printed() {
    for bad in ["../x", "run.1", "a b"] {
        let f = Fixture::new();
        let _r = rotator(&f);
        let out = f.run(&mut step(&f, "0", Some(bad), "21600"));
        assert!(out.status.success(), "{bad}: {}", describe(&out));
        assert_eq!(budget_warnings(&out), vec![NOT_VALID.to_string()], "{bad}");
        assert!(!text(&out.stderr).contains(bad), "{bad:?} printed");
        assert!(budget_records(&f).is_empty(), "{:?}", budget_records(&f));
        assert!(!f.home.join(".amplihack").join("x").exists());
    }
}

#[test]
fn sr10_a_symlink_at_the_record_is_refused_and_never_written_through() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let id = run_id();
    let rerun = f.rerun_dir();
    fs::create_dir_all(&rerun).unwrap();
    for d in [rerun.parent().unwrap(), rerun.as_path()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let target = f.root.join("elsewhere");
    crate::issue_1618_fixture::private_file(&target, "5\n");
    std::os::unix::fs::symlink(&target, budget_record(&f, &id)).unwrap();
    let out = f.run(&mut step(&f, "0", Some(&id), "21600"));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the whole budget: one wait, one retry");
    assert_eq!(budget_warnings(&out), vec![UNUSABLE.to_string()]);
    let link = fs::symlink_metadata(budget_record(&f, &id)).unwrap();
    assert!(
        link.file_type().is_symlink(),
        "the planted link was replaced"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "5\n",
        "written through"
    );
    // The wait's lines and the budget warning; other start-up lines may
    // name the fixture's HOME.
    let root = f.root.to_string_lossy().into_owned();
    for l in text(&out.stderr)
        .lines()
        .filter(|l| l.contains("usage_limit_wait") || l.contains("usage-limit"))
    {
        assert!(!l.contains(&root), "a path leaked into {l:?}");
    }
}

#[test]
fn sr13_a_limit_refused_before_the_budget_check_touches_no_record() {
    for id in [Some(run_id()), None] {
        let f = Fixture::new();
        let _r = rotator(&f);
        let mut c = step(&f, "0", id.as_deref(), "600");
        c.env("LIMIT_MESSAGE", SPEND_TEXT);
        let out = f.run(&mut c);
        assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
        assert_eq!(events(&out), ["skipped reason=spend_limit"]);
        assert!(budget_records(&f).is_empty(), "{:?}", budget_records(&f));
        assert!(
            budget_warnings(&out).is_empty(),
            "{:?}",
            budget_warnings(&out)
        );

        let f = Fixture::new();
        let _r = rotator(&f);
        let mut c = step(&f, "0", id.as_deref(), "600");
        c.env("AMPLIHACK_USAGE_LIMIT_WAIT", "0");
        let out = f.run(&mut c);
        assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
        assert!(budget_records(&f).is_empty());
        assert!(
            budget_warnings(&out).is_empty(),
            "{:?}",
            budget_warnings(&out)
        );
    }
}

#[test]
fn a_zero_budget_refuses_every_wait() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let id = run_id();
    let out = f.run(&mut step(&f, "0", Some(&id), "0"));
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no wait and no retry");
    assert_lines(
        &raw_events(&out),
        &["skipped reason=budget_spent step=\\d+"],
    );
    assert_eq!(
        line_after(&out, "skipped reason=budget_spent"),
        Some(run_budget_note(0))
    );
    let record = budget_record(&f, &id);
    assert_eq!(fs::read(&record).unwrap(), b"", "created at the check");
}

#[test]
fn invalid_budget_variables_warn_without_their_values_and_the_defaults_apply() {
    let f = Fixture::new();
    let _r = rotator(&f);
    let mut c = step(&f, "0", Some(&run_id()), "6h-VALUE-1761");
    c.env(WAITS_VAR, "lots-VALUE-1761");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let w = warnings(&out);
    for var in [BUDGET_VAR, WAITS_VAR] {
        assert_eq!(
            w.iter().filter(|l| l.contains(var)).count(),
            1,
            "{var}: {w:?}"
        );
    }
    assert!(
        !text(&out.stderr).contains("VALUE-1761"),
        "a value was quoted"
    );
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "{:?}",
        events(&out)
    );
}
