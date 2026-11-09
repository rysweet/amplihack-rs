//! Issue #2030: the Codex option tables in `codex_exec_args.rs` match the
//! saved codex-cli 0.166.0 help text, scope by scope.

use super::codex_exec_args::{
    Arity, EXEC, ExecLine, FORK, Opt, RESUME, REVIEW, ROOT, Scope, TOP_REVIEW, parse_exec_line,
};
use std::collections::BTreeSet;

type Row = (String, Option<char>, Arity);

/// The options a Codex help text lists: `-x, --name <VALUE>...` lines.
fn options_in_help(help: &str) -> BTreeSet<Row> {
    let mut rows = BTreeSet::new();
    for line in help.lines() {
        let indent = line.len() - line.trim_start().len();
        let line = line.trim();
        if !(2..=6).contains(&indent) || !line.starts_with('-') {
            continue;
        }
        let (short, rest) = match line.strip_prefix('-').and_then(|r| r.split_once(", ")) {
            Some((s, rest)) if s.len() == 1 => (s.chars().next(), rest),
            _ => (None, line),
        };
        let mut words = rest.split_whitespace();
        let long = words
            .next()
            .and_then(|w| w.strip_prefix("--"))
            .unwrap_or_else(|| panic!("no long name: {line}"));
        let arity = match words.next() {
            None => Arity::Flag,
            Some(v) if v.ends_with("...") => Arity::Many,
            Some(_) => Arity::One,
        };
        rows.insert((long.to_string(), short, arity));
    }
    rows
}

fn listed(table: &[Opt]) -> BTreeSet<Row> {
    table
        .iter()
        .filter(|o| !o.3)
        .map(|o| (o.0.to_string(), o.1, o.2))
        .collect()
}

#[test]
fn tables_match_the_codex_0_166_0_help_text() {
    for (name, table, help) in [
        (
            "root",
            ROOT,
            include_str!("fixtures/codex-root-help-0.166.0.txt"),
        ),
        (
            "exec",
            EXEC,
            include_str!("fixtures/codex-exec-help-0.166.0.txt"),
        ),
        (
            "exec review",
            REVIEW,
            include_str!("fixtures/codex-exec-review-help-0.166.0.txt"),
        ),
        (
            "review",
            TOP_REVIEW,
            include_str!("fixtures/codex-review-help-0.166.0.txt"),
        ),
        (
            "exec resume",
            RESUME,
            include_str!("fixtures/codex-exec-resume-help-0.166.0.txt"),
        ),
        (
            "exec fork",
            FORK,
            include_str!("fixtures/codex-exec-fork-help-0.166.0.txt"),
        ),
    ] {
        let from_help = options_in_help(help);
        assert!(from_help.len() > 8, "{name}: help text not parsed");
        let from_table = listed(table);
        assert_eq!(
            from_table.difference(&from_help).collect::<Vec<_>>(),
            Vec::<&Row>::new(),
            "{name}: in the table, not in the help"
        );
        assert_eq!(
            from_help.difference(&from_table).collect::<Vec<_>>(),
            Vec::<&Row>::new(),
            "{name}: in the help, not in the table"
        );
    }
}

#[test]
fn image_is_variadic_only_under_plain_exec() {
    let arity = |t: &[Opt]| t.iter().find(|o| o.0 == "image").map(|o| o.2);
    assert_eq!(arity(EXEC), Some(Arity::Many));
    assert_eq!(arity(RESUME), Some(Arity::One));
    assert_eq!(arity(FORK), Some(Arity::One));
    assert_eq!(arity(REVIEW), None);
}

fn parse(items: &[&str]) -> Option<ExecLine> {
    let args: Vec<String> = items.iter().map(|a| (*a).to_string()).collect();
    parse_exec_line(&args).unwrap_or_else(|e| panic!("{items:?}: {e:#}"))
}

#[test]
fn scopes_and_positionals() {
    let line = parse(&["exec", "review", "--commit", "abc123"]).unwrap();
    assert_eq!((line.scope, line.positionals), (Scope::Review, vec![]));
    let line = parse(&["exec", "review", "--base", "main", "focus"]).unwrap();
    assert_eq!(line.positionals, vec![4]);
    let line = parse(&["exec", "fork", "id", "-i", "a.png", "task"]).unwrap();
    assert_eq!((line.scope, line.positionals), (Scope::Fork, vec![2, 5]));
    let line = parse(&["exec", "-i", "a.png", "b.png", "-", "--json"]).unwrap();
    assert_eq!((line.positionals, line.widest_image), (vec![4], 2));
    let line = parse(&["exec", "-ia.png", "task"]).unwrap();
    assert_eq!((line.positionals, line.widest_image), (vec![2], 1));
    let line = parse(&["exec", "resume", "--last", "task"]).unwrap();
    assert!(line.last);
    let line = parse(&["exec", "--cyber-access-program", "standard", "task"]).unwrap();
    assert_eq!(line.positionals, vec![3]);
    let line = parse(&["-c", "x=1", "e", "-mgpt", "--", "--not-an-option"]).unwrap();
    assert_eq!((line.scope, line.positionals), (Scope::Exec, vec![5]));
    assert!(parse(&["exec", "help", "review"]).is_none());
    assert!(parse(&["--", "exec"]).is_none());
    assert!(parse(&["login"]).is_none());
    let line = parse(&["--profile", "p", "review", "--base", "main", "focus"]).unwrap();
    assert_eq!((line.scope, line.positionals), (Scope::TopReview, vec![5]));
    // Top-level `resume` and `fork` are interactive: not parsed.
    assert!(parse(&["resume", "id", "task"]).is_none());
    assert!(parse(&["fork", "id", "task"]).is_none());
    assert!(parse(&["--", "review"]).is_none());
    // An unknown root option with no `exec` after it: not exec.
    assert!(parse(&["--new-root-flag", "hello"]).is_none());
}

#[test]
fn unknown_option_followed_by_a_word_is_refused_without_the_word() {
    for items in [
        vec!["exec", "--new-option", "SECRET-WORD"],
        vec!["exec", "review", "--sandbox", "SECRET-WORD"],
        vec!["exec", "-Z", "SECRET-WORD"],
        vec!["--new-root-flag", "v", "exec", "SECRET-WORD"],
        vec!["--new-root-flag", "v", "review", "SECRET-WORD"],
    ] {
        let args: Vec<String> = items.iter().map(|a| (*a).to_string()).collect();
        let err = parse_exec_line(&args).expect_err("refused");
        let message = format!("{err:#}");
        assert!(message.contains("does not know whether"), "{message}");
        assert!(!message.contains("SECRET-WORD"), "{message}");
    }
    // Followed by an option, or written with `=`: no ambiguity.
    let line = parse(&["exec", "--new-option", "--json", "task"]).unwrap();
    assert_eq!(line.positionals, vec![3]);
    let line = parse(&["exec", "--new-option=v", "task"]).unwrap();
    assert_eq!(line.positionals, vec![2]);
}

#[test]
fn missing_values_are_refused() {
    for items in [
        vec!["exec", "review", "--commit"],
        vec!["exec", "-i"],
        vec!["exec", "-i", "--json"],
        vec!["exec", "fork", "id", "-i"],
    ] {
        let args: Vec<String> = items.iter().map(|a| (*a).to_string()).collect();
        assert!(parse_exec_line(&args).is_err(), "{items:?}");
    }
}
