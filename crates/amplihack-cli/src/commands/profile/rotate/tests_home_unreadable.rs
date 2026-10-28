//! A `~/.claude.json` that stays unreadable (#1702 review, round 2). One bad
//! read is Claude rewriting the file, so the first cycles only ALERT and say
//! the next cycle reads it again. Once the spell has lasted
//! `HOME_UNREADABLE_GRACE` seconds the ALERT names the file's problem and
//! gives fix lines that keep L's login, `rotate status` shows them, and they
//! stay until the file has an account again. Fixture: `tests_handfix.rs`
//! (the ledger names L; L has no login file, its login is in `~/.claude`).

use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::guide::home_stuck_head;
use super::handfix::{Step, preview};
use super::identify::{LiveId, home_problem, live};
use super::ledger::HOME_UNREADABLE_GRACE;
use super::log::{msg, sh_quote};
use super::tests_handfix::{C, Fx, L, T, acct, fx, seam};
use super::tests_handfix_cycle::{ctx, m_spent, never};
use super::tests_swap::{ino_at, n, private};
use std::fs;
use std::path::Path;

const G: i64 = HOME_UNREADABLE_GRACE;

/// Each way `~/.claude.json` can hold no account, and what the ALERT says.
/// `None`: the file is removed.
const CASES: &[(Option<&str>, &str)] = &[
    (None, "it does not exist"),
    (Some("{"), "it is not valid JSON (empty or cut short)"),
    (Some(""), "it is not valid JSON (empty or cut short)"),
    (Some("[]"), "it is not a JSON object"),
    (Some(r#"{"projects":{}}"#), "it has no oauthAccount block"),
    (
        Some(r#"{"oauthAccount":{"accountUuid":""}}"#),
        "its oauthAccount block has no usable accountUuid or emailAddress",
    ),
];

fn spoil(f: &Fx, body: Option<&str>) {
    let p = f.home.join(".claude.json");
    match body {
        Some(b) => private(&p, b),
        None => fs::remove_file(&p).unwrap(),
    }
}

fn q(p: &Path) -> String {
    sh_quote(p).unwrap()
}

/// The escalated lines, built from the documented wording.
fn stuck_lines(f: &Fx, problem: &str) -> Vec<String> {
    vec![
        home_stuck_head(&n(L), problem),
        format!(
            "fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if {} then holds an account, the next cycle goes on",
            q(&f.home.join(".claude.json"))
        ),
        format!(
            "fix 2: or give {L} its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e {} && mv -n {} {}/ && rm {}; then run claude with no CLAUDE_CONFIG_DIR and /login",
            q(&f.slot(L)),
            q(&f.live()),
            q(&f.root.join(L)),
            q(&f.rs.ledger)
        ),
    ]
}

/// The shell command in `fix 2`, between `then run: ` and `; then run`.
/// Run `cmd` under `/bin/sh` with a fixed PATH. Other tests in this binary
/// briefly point the process-wide PATH at a temporary directory (under
/// `home_env_lock`), and a bare `sh` lookup during that window fails.
fn run_sh(cmd: &str) -> std::process::ExitStatus {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .status()
        .unwrap()
}

fn fix2_command(line: &str) -> String {
    let start = line.find("then run: ").unwrap() + "then run: ".len();
    let end = line.rfind("; then run claude").unwrap();
    line[start..end].to_string()
}

#[cfg(target_os = "linux")]
#[test]
fn a_home_account_that_stays_unreadable_escalates_to_fix_lines() {
    for (body, problem) in CASES {
        let f = fx(true, false);
        spoil(&f, *body);
        assert_eq!(home_problem(&f.home), *problem);
        assert_eq!(
            live(&f.store, &f.rs, &f.home).unwrap().0,
            LiveId::HomeUnreadable(n(L))
        );
        // Inside the grace: no guidance, the spell's start is kept.
        for t in [T, T + 60, T + G - 1] {
            assert_eq!(f.step(&seam(), t), Step::Clear, "{problem} at +{}", t - T);
            let l = f.ledger();
            assert_eq!(l.home_unreadable_since, Some(T), "{problem}");
            assert_eq!(l.guidance, None, "{problem}");
        }
        assert!(f.logged().is_empty(), "{:?}", f.logged());
        assert!(f.status().is_empty());

        // From the grace on: the problem and real fix lines, every cycle.
        let want = stuck_lines(&f, problem);
        for t in [T + G, T + G + 600] {
            assert_eq!(f.step(&seam(), t), Step::Guided, "{problem}");
            assert_eq!(f.ledger().guidance.as_ref(), Some(&want), "{problem}");
        }
        assert_eq!(f.logged(), [want.clone(), want.clone()].concat());
        let status = f.status();
        assert_eq!(status.len(), 3, "{status:?}");
        assert!(status[0].starts_with("alert: ~/.claude.json has had no account"));
        assert!(status[0].contains(problem), "{status:?}");
        assert!(
            want[1].matches("/login").count() == 1 && want[1].contains("without /login"),
            "fix 1 never logs in over L's login"
        );
        f.assert_nothing_moved();
    }
}

/// A cycle that reads an account ends the spell: the next unreadable read
/// starts a new one, so a file Claude is merely rewriting never escalates.
#[cfg(target_os = "linux")]
#[test]
fn a_readable_account_in_between_starts_the_spell_again() {
    let f = fx(true, false);
    let json = f.home.join(".claude.json");
    spoil(&f, Some("{"));
    assert_eq!(f.step(&seam(), T), Step::Clear);
    // L's own account: the ledger is right, no hand change.
    private(&json, &acct("u-rs", "rs@example.com"));
    assert_eq!(f.step(&seam(), T + 100), Step::Clear);
    assert_eq!(f.ledger().home_unreadable_since, None);
    spoil(&f, Some("{"));
    assert_eq!(f.step(&seam(), T + G), Step::Clear, "a new spell");
    assert_eq!(f.ledger().home_unreadable_since, Some(T + G));
    assert_eq!(f.ledger().guidance, None);
    // A start in the future (the clock went back) counts from now.
    let mut l = f.ledger();
    l.home_unreadable_since = Some(T + 10 * G);
    f.rs.save_ledger(&l).unwrap();
    assert_eq!(f.step(&seam(), T + 2 * G), Step::Clear);
    assert_eq!(f.ledger().home_unreadable_since, Some(T + 2 * G));
}

/// Escalated guidance goes away by itself once the file has an account
/// again; with the 27 Oct hand change behind it, that cycle recovers.
#[cfg(target_os = "linux")]
#[test]
fn escalated_guidance_clears_once_the_file_is_readable() {
    let f = fx(false, false);
    let good = fs::read_to_string(f.home.join(".claude.json")).unwrap();
    spoil(&f, Some(r#"{"projects":{}}"#));
    f.step(&seam(), T);
    assert_eq!(f.step(&seam(), T + G), Step::Guided);
    private(&f.home.join(".claude.json"), &good);
    assert_eq!(f.step(&seam(), T + G + 600), Step::Recovered);
    let l = f.ledger();
    assert_eq!(l.home_unreadable_since, None);
    assert_eq!(l.guidance, None);
    assert!(f.status().iter().all(|s| !s.starts_with("alert:")));
}

/// `--dry-run` shows the escalated ALERT and fix lines and writes nothing.
#[cfg(target_os = "linux")]
#[test]
fn dry_run_previews_the_escalated_guidance() {
    let f = fx(true, false);
    spoil(&f, None);
    f.step(&seam(), T);
    let before = fs::read(&f.rs.ledger).unwrap();
    assert!(
        preview(&f.store, &f.rs, &f.home, seam, T + G - 1)
            .unwrap()
            .is_empty()
    );
    let got = preview(&f.store, &f.rs, &f.home, seam, T + G).unwrap();
    let want = stuck_lines(&f, "it does not exist");
    let head = want[0].strip_prefix("ALERT: ").unwrap();
    assert_eq!(got[0], format!("would alert: {head}"));
    assert_eq!(got[1..], want[1..]);
    assert_eq!(fs::read(&f.rs.ledger).unwrap(), before, "nothing written");
}

/// `fix 2` as printed, run by `sh`: L gets its login back by rename, the
/// ledger goes, and `~/.claude` is left with no login for the `/login`.
/// When L already has a login file, the `test` stops it and nothing moves.
#[cfg(target_os = "linux")]
#[test]
fn fix_two_gives_l_its_login_back_and_never_replaces_one() {
    let f = fx(true, false);
    spoil(&f, Some("{"));
    f.step(&seam(), T);
    f.step(&seam(), T + G);
    let cmd = fix2_command(&f.ledger().guidance.unwrap()[2]);

    let blocker = f.slot(L);
    private(&blocker, "{}");
    let before = ino_at(&blocker);
    let st = run_sh(&cmd);
    assert!(!st.success(), "refused while L has a login file");
    assert_eq!(ino_at(&f.live()), Some(f.h));
    assert_eq!(ino_at(&blocker), before);
    assert!(f.rs.ledger.exists());
    fs::remove_file(&blocker).unwrap();

    let st = run_sh(&cmd);
    assert!(st.success(), "{cmd}");
    assert_eq!(ino_at(&f.slot(L)), Some(f.h), "L's login, by rename");
    assert_eq!(ino_at(&f.live()), None);
    assert!(!f.rs.ledger.exists());
}

/// Through `run_cycle`: the first cycle gives the retry ALERT only; a cycle
/// once the spell is `HOME_UNREADABLE_GRACE` old ends `alerted` with the
/// fix lines in the log and the ledger; after the file is fixed the next
/// cycle recovers and swaps.
#[cfg(target_os = "linux")]
#[test]
fn run_cycle_escalates_a_home_account_that_stays_unreadable() {
    let f = fx(true, true);
    let good = fs::read_to_string(f.home.join(".claude.json")).unwrap();
    spoil(&f, Some(r#"{"projects":{}}"#));
    let mut c = ctx(&f, never);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    assert_eq!(f.ledger().guidance, None);
    let since = f.ledger().home_unreadable_since.expect("spell recorded");
    assert!(f.logged().contains(&msg::home_unreadable(&n(L))));

    // The second cycle, a grace later: the start is moved back instead of
    // sleeping.
    let mut l = f.ledger();
    l.home_unreadable_since = Some(since - G);
    f.rs.save_ledger(&l).unwrap();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    let want = stuck_lines(&f, "it has no oauthAccount block");
    assert_eq!(f.ledger().guidance.as_ref(), Some(&want));
    let log = f.logged();
    assert!(log.ends_with(&want), "{log:?}");
    assert_eq!(f.ledger().last_cycle.unwrap().outcome, "alerted");
    f.assert_nothing_moved();

    private(&f.home.join(".claude.json"), &good);
    c.measure = Some(m_spent);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(ino_at(&f.live()), f.c);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(C));
    assert_eq!((l.guidance, l.home_unreadable_since), (None, None));
    f.assert_each_login_once();
}
