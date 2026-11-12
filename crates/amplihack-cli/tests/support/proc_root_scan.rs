//! Issue #2042: the text scanner behind the guard test
//! `issue_2042_proc_root_guard.rs`. It finds test code that can let an
//! `amplihack profile` or `sessions` command read the host `/proc`.
//!
//! It matches text, skips comment lines, and splits a file into functions at
//! lines that declare one: `fn`, after any visibility (`pub`, `pub(crate)`,
//! `pub(super)`, `pub(in ..)`) and any of `async`, `const` and `unsafe`.
//! Lines before the first function form a chunk of their own. Its rules are
//! the table in `docs/howto/keep-tests-off-the-host-process-table.md`.

/// The rules (a)-(e) of the how-to guide's scanner table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    A,
    B,
    C,
    D,
    E,
}

const VAR: &str = "AMPLIHACK_TEST_PROC_ROOT";

/// Whether `line` declares a function.
pub fn is_fn_start(line: &str) -> bool {
    let mut t = line.trim_start();
    if let Some(rest) = t.strip_prefix("pub") {
        t = match rest.strip_prefix('(') {
            Some(scoped) => scoped.split_once(") ").map_or("", |(_, r)| r),
            None => rest.strip_prefix(' ').unwrap_or(""),
        };
    }
    while let Some(rest) = ["async ", "const ", "unsafe "]
        .iter()
        .find_map(|q| t.strip_prefix(q))
    {
        t = rest;
    }
    t.starts_with("fn ")
}

fn sets_tree(line: &str) -> bool {
    line.contains("reapply_proc_root") || line.contains("PROC_ROOT_ENV") || line.contains(VAR)
}

/// `(line, rule)` for each finding in `src`, 1-based lines, sorted.
pub fn scan(src: &str) -> Vec<(usize, Rule)> {
    let lines: Vec<(usize, &str)> = src
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect();
    // `sessions` reads `/proc` through the same seam (`sessions/live.rs`).
    let mentions_profile = lines
        .iter()
        .any(|(_, l)| l.contains("\"profile\"") || l.contains("\"sessions\""));
    let mut out = Vec::new();
    for &(n, l) in &lines {
        if l.contains("env_remove(") && sets_tree(l) {
            out.push((n, Rule::B));
        }
        if (l.contains("PROC_ROOT_ENV") || l.contains(VAR)) && l.contains("\"/proc\"") {
            out.push((n, Rule::C));
        }
    }
    let mut starts: Vec<usize> = (0..lines.len())
        .filter(|&i| i == 0 || is_fn_start(lines[i].1))
        .collect();
    starts.push(lines.len());
    for w in starts.windows(2) {
        let body = &lines[w[0]..w[1]];
        link_own_findings(body, &mut out);
        let raw_bin = body
            .iter()
            .find(|(_, l)| l.contains("Command::new(") && l.contains("amplihack"));
        let builds = raw_bin.is_some() || body.iter().any(|(_, l)| l.contains("isolated("));
        if !builds || !mentions_profile {
            continue;
        }
        let any_tree = body.iter().any(|(_, l)| sets_tree(l));
        if let Some(&(n, _)) = raw_bin
            && !body.iter().any(|(_, l)| l.contains("isolated("))
            && !any_tree
        {
            out.push((n, Rule::A));
        }
        if let Some(i) = body.iter().rposition(|(_, l)| l.contains("env_clear()"))
            && !body[i..].iter().any(|(_, l)| sets_tree(l))
        {
            out.push((body[i].0, Rule::D));
        }
    }
    out.sort();
    out
}

/// Rule (e) within one function: `link_own(..)` with an integer literal, or
/// in a function that neither spawns a process nor runs `rotate start`. A
/// quoted "start" on any line counts, so rustfmt's wrapping cannot hide it.
fn link_own_findings(body: &[(usize, &str)], out: &mut Vec<(usize, Rule)>) {
    let starts_something = body
        .iter()
        .any(|(_, l)| l.contains(".spawn()") || l.contains("\"start\""));
    for &(n, l) in body {
        if l.contains("link_own(") && !l.contains("fn link_own") {
            let arg = l.split("link_own(").nth(1).unwrap_or("");
            if arg.starts_with(|c: char| c.is_ascii_digit()) || !starts_something {
                out.push((n, Rule::E));
            }
        }
    }
}
