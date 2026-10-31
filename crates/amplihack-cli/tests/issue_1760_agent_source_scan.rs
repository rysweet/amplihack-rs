//! Issue #1760, R8, R9.1, R10.2, D23, S16, S20 and S27: source and docs
//! guards for `amplihack agent`. The module list exists; every new file is
//! 300 lines or fewer; every unit-test file is wired; only `tmux.rs` runs
//! tmux, and `server.rs` under `systemd-run` (G1); no shell, no tmux kill or `remain-on-exit`, no `open_private_append`
//! or `create_dir_all`, no #1569/#1618 identifiers; the `/proc` test seam is
//! read in one place and never documented; the docs use no retired names, and
//! every documented `amplihack agent` and `amplihack claude --supervise`
//! command parses with the real CLI.

#![cfg(target_os = "linux")]

use amplihack_cli::Cli;
use std::fs;
use std::path::{Path, PathBuf};

const MODULES: &str = "mod validate registry store procfs cmdline env_record sessions scan \
    markers socket_check tmux tmux_parse server dirty launch report pass resume supervise discover \
    register wrapper";
const UNIT_TESTS: &str = "tests_validate tests_registry tests_store tests_procfs tests_cmdline \
    tests_env_record tests_env_drift tests_sessions tests_scan tests_markers tests_tmux \
    tests_launch tests_report tests_pass tests_supervise tests_server tests_validate_hardening";
const DOCS: &str = "docs/howto/supervise-agent-sessions.md docs/reference/agent-command.md";

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn agent_dir() -> PathBuf {
    manifest().join("src/commands/agent")
}
fn hooks_dir() -> PathBuf {
    manifest().join("../amplihack-hooks/src/session_stop")
}
fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

/// Every `.rs` file directly in `src/commands/agent/`, sorted.
fn agent_files() -> Vec<PathBuf> {
    let entries = fs::read_dir(agent_dir()).into_iter().flatten().flatten();
    let mut v: Vec<_> = entries
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    v.sort();
    v
}

/// The two pages and their text.
fn docs() -> Vec<(&'static str, String)> {
    DOCS.split_whitespace()
        .map(|d| (d, read(&manifest().join("../..").join(d))))
        .collect()
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// The non-test source files of the module.
fn sources() -> Vec<(String, String)> {
    let srcs: Vec<_> = agent_files()
        .iter()
        .filter(|p| !name(p).starts_with("tests_"))
        .map(|p| (name(p), read(p)))
        .collect();
    assert!(
        srcs.len() >= MODULES.split_whitespace().count(),
        "src/commands/agent/ is missing"
    );
    srcs
}

/// `src` without `//` comments, so prose may name what the code never does.
fn code(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        out.push_str(line.split(" // ").next().unwrap_or(line));
        out.push('\n');
    }
    out
}

#[test]
fn the_planned_modules_exist() {
    let missing: Vec<_> = MODULES
        .split_whitespace()
        .filter(|m| !agent_dir().join(format!("{m}.rs")).is_file())
        .collect();
    assert!(missing.is_empty(), "not in agent/: {missing:?}");
}

/// D23: 300 lines or fewer for every new file, source and test.
#[test]
fn every_new_file_has_300_lines_or_fewer() {
    let mut files = agent_files();
    let planned = MODULES
        .split_whitespace()
        .chain(UNIT_TESTS.split_whitespace());
    assert!(files.len() >= planned.count(), "{files:?}");
    for f in ["agent_marker.rs", "agent_marker_tests.rs"] {
        assert!(hooks_dir().join(f).is_file(), "{f} is missing");
        files.push(hooks_dir().join(f));
    }
    for e in fs::read_dir(manifest().join("tests")).unwrap().flatten() {
        if name(&e.path()).starts_with("issue_1760_") {
            files.push(e.path());
        }
    }
    let over: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let n = read(f).lines().count();
            (n > 300).then(|| format!("{} ({n} lines)", f.display()))
        })
        .collect();
    assert!(over.is_empty(), "files over 300 lines: {over:#?}");
}

/// R9.1: unit tests are `#[cfg(test)] #[path]` modules, so each must be
/// declared or it never compiles.
#[test]
fn every_unit_test_file_is_wired() {
    let mut tests: Vec<String> = agent_files()
        .iter()
        .map(|p| name(p))
        .filter(|n| n.starts_with("tests_"))
        .collect();
    for t in UNIT_TESTS.split_whitespace() {
        assert!(tests.contains(&format!("{t}.rs")), "{t}.rs is missing");
    }
    tests.sort();
    let srcs = sources();
    let wired = |src: &str, file: &str| {
        let lines: Vec<&str> = src.lines().map(str::trim).collect();
        lines.iter().enumerate().any(|(i, l)| {
            *l == format!("#[path = \"{file}\"]")
                && lines[i.saturating_sub(3)..i].contains(&"#[cfg(test)]")
        })
    };
    let unwired: Vec<_> = tests
        .iter()
        .filter(|t| !srcs.iter().any(|(_, s)| wired(s, t)))
        .collect();
    assert!(
        unwired.is_empty(),
        "not declared as #[cfg(test)] #[path]: {unwired:?}"
    );
    let marker = read(&hooks_dir().join("agent_marker.rs"));
    assert!(
        wired(&marker, "agent_marker_tests.rs"),
        "agent_marker_tests.rs is not wired"
    );
    let stop = read(&hooks_dir().join("mod.rs"));
    assert!(
        stop.contains("#[cfg(unix)]\nmod agent_marker;\n"),
        "session_stop/mod.rs"
    );
}

#[test]
fn only_tmux_rs_runs_tmux() {
    let srcs = sources();
    let with = |needle: &str| -> Vec<&str> {
        let hits = srcs.iter().filter(|(_, s)| code(s).contains(needle));
        hits.map(|(n, _)| n.as_str()).collect()
    };
    assert_eq!(with("OsStr::new(\"tmux\")"), ["tmux.rs"]);
    assert_eq!(with("\"systemd-run\""), ["server.rs"]);
    assert_eq!(with("tmux::client("), ["server.rs"]);
}

#[test]
fn no_shell_no_kill_and_no_unsafe_file_helpers() {
    const BANNED: &str = "kill-server|kill-session|remain-on-exit|sh -c|bash -c|\
        Command::new(\"sh\")|Command::new(\"bash\")|Command::new(\"/bin/sh\")|\
        open_private_append|create_dir_all|run_supervised|supervise_gate|\
        AMPLIHACK_TEST_AGENT_LAUNCHER";
    let mut found = Vec::new();
    for (n, s) in sources() {
        let c = code(&s);
        found.extend(
            BANNED
                .split('|')
                .filter(|b| c.contains(*b))
                .map(|b| format!("{n}: {b}")),
        );
    }
    assert!(found.is_empty(), "{found:#?}");
}

/// S27: the seam is read in exactly one place.
#[test]
fn the_proc_test_seam_is_read_once_and_undocumented() {
    let uses: Vec<String> = sources()
        .into_iter()
        .flat_map(|(n, s)| {
            let k = code(&s).matches("AMPLIHACK_TEST_PROC_ROOT").count();
            std::iter::repeat_n(n, k)
        })
        .collect();
    assert_eq!(uses, ["procfs.rs"]);
}

/// R10.2: no name from the earlier drafts; S27: no test seam for users.
#[test]
fn the_docs_use_no_retired_names_and_no_test_seam() {
    let retired = "agent run|agent service|--batch-size|--batch-gap|--grace|--dry-run|\
        AMPLIHACK_TEST_PROC_ROOT";
    for (d, text) in docs() {
        for r in retired.split('|') {
            assert!(!text.contains(r), "{d} mentions {r:?}");
        }
    }
}

/// `amplihack ...` commands from ```bash blocks and `! ` lines, with
/// backslash-continued lines joined.
fn documented_commands(text: &str) -> Vec<String> {
    let (mut out, mut pending) = (Vec::new(), String::new());
    let mut fence: Option<bool> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("```") {
            fence = fence.is_none().then_some(rest == "bash");
            continue;
        }
        let cmd = match (t.strip_prefix("! "), fence) {
            (Some(c), _) => c,
            (None, Some(true)) => t,
            _ => continue,
        };
        if let Some(c) = cmd.strip_suffix('\\') {
            pending.push_str(c);
            pending.push(' ');
            continue;
        }
        pending.push_str(cmd);
        let full = std::mem::take(&mut pending);
        if full.starts_with("amplihack ") {
            out.push(full);
        }
    }
    out
}

/// Splits like a shell for these docs: whitespace, single and double quotes.
fn split(cmd: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for ch in cmd.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

#[test]
fn every_documented_command_parses() {
    let mut seen = std::collections::BTreeSet::new();
    for (d, text) in docs() {
        for cmd in documented_commands(&text) {
            let cmd = cmd
                .replace("\"$a:main\"", "area-search:main")
                .replace("\"$a\"", "area-search");
            let argv = split(&cmd);
            let kind = match argv.get(1).map(String::as_str) {
                Some("agent") => argv.get(2).cloned().unwrap_or_default(),
                Some("claude") if argv.iter().any(|a| a == "--supervise") => "claude".into(),
                _ => continue,
            };
            if let Err(e) = Cli::try_parse_from(&argv) {
                panic!("{d}: `{cmd}` does not parse:\n{e}");
            }
            seen.insert(kind);
        }
    }
    let want = ["claude", "register", "status", "supervise", "unregister"];
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), want);
}
