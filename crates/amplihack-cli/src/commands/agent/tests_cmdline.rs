//! Unit tests for `cmdline.rs`: the launch options read from a `claude`
//! command line (R3.2, design 4.5) and from an `amplihack claude` ancestor,
//! the print-mode flag (B2), `--fork-session` (B3) and values dropped by the
//! S7 rule.
//!
//! API under test:
//! - `parse_claude(argv: &[String]) -> (ClaudeOpts, Vec<String>)`: `argv`
//!   includes the program; the `Vec` holds warnings of the form
//!   `ignored FLAG from the claude command line (N bytes): RULE` (`main`
//!   adds `warning: `)
//! - `ClaudeOpts { model, effort, permission_mode: Option<String>,
//!   skip_permissions: bool, add_dirs: Vec<String>, resume: Option<String>,
//!   fork_session: bool, print: bool }`
//! - `resume_as_conversation(o: &ClaudeOpts) -> Option<&str>`: the
//!   `--resume` id, or `None` under `--fork-session`
//! - `parse_amplihack_claude(argv: &[String]) -> Option<AmplihackClaude>`,
//!   `AmplihackClaude { profile: Option<String>, subprocess_safe: bool,
//!   no_reflection: bool, supervise: Option<String>, user_args: Vec<String> }`

use super::cmdline::*;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

#[test]
fn reads_both_spellings_of_each_option() {
    let (o, w) = parse_claude(&v(&[
        "claude",
        "--model",
        "claude-opus-5-5",
        "--effort=high",
        "--permission-mode=acceptEdits",
        "--dangerously-skip-permissions",
        "--add-dir",
        "/a",
        "/b",
        "--add-dir=/c",
        "--resume",
        ID,
    ]));
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(o.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(o.effort.as_deref(), Some("high"));
    assert_eq!(o.permission_mode.as_deref(), Some("acceptEdits"));
    assert!(
        o.skip_permissions,
        "the parser keeps both; the entry applies R5.6"
    );
    assert_eq!(o.add_dirs, ["/a", "/b", "/c"]);
    assert_eq!(o.resume.as_deref(), Some(ID));
    assert!(!o.fork_session && !o.print);
    let (o, _) = parse_claude(&v(&[
        "claude",
        "--model=m",
        "--effort",
        "low",
        "--permission-mode",
        "plan",
    ]));
    assert_eq!(
        (o.model.as_deref(), o.effort.as_deref()),
        (Some("m"), Some("low"))
    );
    assert_eq!(o.permission_mode.as_deref(), Some("plan"));
}

#[test]
fn the_last_scalar_value_wins() {
    let (o, _) = parse_claude(&v(&[
        "claude",
        "--model",
        "a",
        "--model=b",
        "--effort=x",
        "--effort",
        "y",
    ]));
    assert_eq!(
        (o.model.as_deref(), o.effort.as_deref()),
        (Some("b"), Some("y"))
    );
}

#[test]
fn add_dir_is_variadic_until_the_next_option() {
    let (o, _) = parse_claude(&v(&["claude", "--add-dir", "/a", "/b", "--model", "m"]));
    assert_eq!(o.add_dirs, ["/a", "/b"]);
    assert_eq!(o.model.as_deref(), Some("m"));
    let (o, _) = parse_claude(&v(&["claude", "--add-dir", "--model", "m"]));
    assert!(o.add_dirs.is_empty());
    assert_eq!(o.model.as_deref(), Some("m"));
}

#[test]
fn resume_forms() {
    let (o, _) = parse_claude(&v(&["claude", "-r", ID]));
    assert_eq!(o.resume.as_deref(), Some(ID));
    let (o, _) = parse_claude(&v(&["claude", format!("--resume={ID}").as_str()]));
    assert_eq!(o.resume.as_deref(), Some(ID));
    let (o, _) = parse_claude(&v(&["claude", "--resume"]));
    assert_eq!(o.resume, None, "--resume with no value opens the picker");
    let (o, _) = parse_claude(&v(&["claude", "--resume", "--model", "m"]));
    assert_eq!((o.resume, o.model.as_deref()), (None, Some("m")));
}

#[test]
fn a_forked_session_does_not_name_its_conversation() {
    let (o, _) = parse_claude(&v(&["claude", "--resume", ID]));
    assert_eq!(resume_as_conversation(&o), Some(ID));
    let (o, _) = parse_claude(&v(&["claude", "--resume", ID, "--fork-session"]));
    assert!(o.fork_session);
    assert_eq!(o.resume.as_deref(), Some(ID));
    assert_eq!(
        resume_as_conversation(&o),
        None,
        "B3: the id names the parent"
    );
}

#[test]
fn print_mode_is_seen() {
    assert!(parse_claude(&v(&["claude", "-p", "do it"])).0.print);
    assert!(parse_claude(&v(&["claude", "--print"])).0.print);
    assert!(!parse_claude(&v(&["claude", "--", "-p"])).0.print);
}

#[test]
fn parsing_stops_at_a_bare_double_dash() {
    let (o, _) = parse_claude(&v(&[
        "claude", "--effort", "low", "--", "--model", "m", "-p",
    ]));
    assert_eq!(o.effort.as_deref(), Some("low"));
    assert_eq!(o.model, None);
    assert!(!o.print);
}

#[test]
fn other_options_values_are_never_interpreted() {
    let prompt = "Coordinator.\n--model evil\n-r 1234 --dangerously-skip-permissions";
    let argv = v(&["claude", "--append-system-prompt", prompt, "--model", "m"]);
    let (o, w) = parse_claude(&argv);
    assert_eq!(o.model.as_deref(), Some("m"));
    assert_eq!(o.resume, None);
    assert!(!o.skip_permissions);
    assert!(w.is_empty(), "{w:?}");
    let node = "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js";
    let (o, _) = parse_claude(&v(&["node", node, "--effort", "max"]));
    assert_eq!(o.effort.as_deref(), Some("max"));
}

#[test]
fn bad_values_are_dropped_with_a_warning_that_gives_only_the_length() {
    let (o, w) = parse_claude(&v(&[
        "claude",
        "--model=-x",
        "--effort=hi\u{7}gh",
        "--add-dir=/ok",
        "--resume=--print",
        "--permission-mode",
        "evil\u{1b}mode",
    ]));
    assert_eq!((o.model, o.effort, o.permission_mode), (None, None, None));
    assert_eq!(o.add_dirs, ["/ok"]);
    assert_eq!(o.resume, None, "S7: an id never starts with '-'");
    assert!(!o.print, "a value of --resume= is not an option");
    assert_eq!(w.len(), 4, "{w:?}");
    for prefix in [
        "ignored --model from the claude command line (2 bytes): ",
        "ignored --effort from the claude command line (5 bytes): ",
        "ignored --resume from the claude command line (7 bytes): ",
        "ignored --permission-mode from the claude command line (9 bytes): ",
    ] {
        assert!(
            w.iter().any(|m| m.starts_with(prefix)),
            "{prefix:?} in {w:?}"
        );
    }
    for value in [
        "-x",
        "hi\u{7}gh",
        "--print",
        "evil\u{1b}mode",
        "\u{7}",
        "\u{1b}",
    ] {
        assert!(
            w.iter().all(|m| !m.contains(value)),
            "a warning repeated {value:?}: {w:?}"
        );
    }
}

#[test]
fn amplihack_ancestors() {
    let a = parse_amplihack_claude(&v(&[
        "/home/u/.local/bin/amplihack",
        "claude",
        "--profile",
        "work",
        "--subprocess-safe",
        "--no-reflection",
        "--supervise",
        "coord",
        "--",
        "--dangerously-skip-permissions",
        "--model",
        "m",
    ]))
    .expect("an amplihack claude process");
    assert_eq!(a.profile.as_deref(), Some("work"));
    assert!(a.subprocess_safe && a.no_reflection);
    assert_eq!(a.supervise.as_deref(), Some("coord"));
    assert_eq!(
        a.user_args,
        ["--dangerously-skip-permissions", "--model", "m"]
    );

    let a = parse_amplihack_claude(&v(&["amplihack", "launch", "--profile=w2"])).expect("launch");
    assert_eq!(a.profile.as_deref(), Some("w2"));
    assert!(a.user_args.is_empty() && !a.subprocess_safe && a.supervise.is_none());

    let a = parse_amplihack_claude(&v(&["amplihack", "claude", "--", "--profile", "x"])).unwrap();
    assert_eq!(a.profile, None, "--profile after -- belongs to claude");

    let a = parse_amplihack_claude(&v(&[
        "amplihack",
        "claude",
        "--checkout-repo",
        "o/r",
        "--max-turns",
        "5",
        "--model",
        "m",
    ]))
    .unwrap();
    assert_eq!(
        a.user_args,
        ["--model", "m"],
        "value options are skipped with their value"
    );

    let a = parse_amplihack_claude(&v(&["amplihack", "claude", "--profile", "../x"])).unwrap();
    assert_eq!(
        a.profile, None,
        "S8: a discovered profile must pass the name rule"
    );

    for not in [
        &["amplihack", "recipe", "run"][..],
        &["python", "amplihack", "claude"],
        &["claude"],
        &[],
    ] {
        assert!(parse_amplihack_claude(&v(not)).is_none(), "{not:?}");
    }
}
