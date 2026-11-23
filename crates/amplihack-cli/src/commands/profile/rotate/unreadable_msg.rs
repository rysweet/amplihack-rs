//! Issue #2820: the rotate log's line for a process of this user whose
//! environment the liveness scan could not read. Pure formatting; see
//! "Unreadable process lines" in docs/reference/profile-command.md.
//!
//! The arguments are under the control of any process of this user, so the
//! order of work is mask, then escape, then cut: an environment assignment
//! never shows its value, a cut never splits an escape, and one process is
//! always exactly one line that cannot forge another. The environment itself
//! is never read, so it cannot reach this text.

use crate::commands::profile::auto_switch::{Unreadable, Verdict, assignment_name};

/// The most process lines [`lines`] gives; the rest are counted in one
/// summary line. Every process is still judged: this caps the log only.
pub const MAX_LINES: usize = 32;
/// The most characters of the command line shown, counted after escaping.
const MAX_COMMAND: usize = 120;
/// The most characters of the executable shown, counted after escaping.
const MAX_EXE: usize = 32;

/// `unreadable process <pid> (<exe>) "<command>": <verdict>`.
pub fn line(u: &Unreadable) -> String {
    let command: Vec<String> = u.argv.iter().map(|a| mask(a)).collect();
    format!(
        "unreadable process {} ({}) \"{}\": {}",
        u.pid,
        cut(&u.exe, MAX_EXE),
        cut(&command.join(" "), MAX_COMMAND),
        verdict(&u.verdict)
    )
}

/// One [`line`] for each of the first [`MAX_LINES`] processes, in the order
/// given (the scan's pid order), then `unreadable processes: <N> more not
/// listed` when there are more.
pub fn lines(all: &[Unreadable]) -> Vec<String> {
    let mut out: Vec<String> = all.iter().take(MAX_LINES).map(line).collect();
    if all.len() > MAX_LINES {
        out.push(format!(
            "unreadable processes: {} more not listed",
            all.len() - MAX_LINES
        ));
    }
    out
}

fn verdict(v: &Verdict) -> String {
    match v {
        Verdict::Counted(names) => {
            let names: Vec<String> = names
                .iter()
                .map(|n| n.chars().map(escape).collect())
                .collect();
            format!("counted as a session on {}", names.join(", "))
        }
        Verdict::NotClaude => "not counted (not a Claude executable)".to_string(),
        Verdict::ProfileUnknown => "not counted (profile cannot be told)".to_string(),
    }
}

/// `NAME=value` as `NAME=<hidden>`, and `--flag=NAME=value` as
/// `--flag=NAME=<hidden>`: an environment entry handed on. A plain
/// `--flag=value` is shown as it is.
fn mask(arg: &str) -> String {
    if let Some(name) = assignment_name(arg) {
        return format!("{name}=<hidden>");
    }
    if arg.starts_with('-')
        && let Some((flag, rest)) = arg.split_once('=')
        && let Some(name) = assignment_name(rest)
    {
        return format!("{flag}={name}=<hidden>");
    }
    arg.to_string()
}

/// One character as its display unit: `"` and `\` behind a backslash, an
/// ASCII control character or DEL as `\xNN`, any other control character
/// as `\u{NN}`, the rest as it is.
fn escape(c: char) -> String {
    match c {
        '"' => "\\\"".to_string(),
        '\\' => "\\\\".to_string(),
        c if u32::from(c) < 0x20 || c == '\x7f' => format!("\\x{:02x}", u32::from(c)),
        c if c.is_control() => format!("\\u{{{:x}}}", u32::from(c)),
        c => c.to_string(),
    }
}

/// `s` escaped, cut to at most `max` characters between whole units, with
/// `...` after a cut. Only the characters shown are escaped, so cutting a
/// command line of 64 KiB costs no more than cutting one of 120 characters.
fn cut(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for unit in s.chars().map(escape) {
        let width = unit.chars().count();
        if used + width > max {
            out.push_str("...");
            break;
        }
        used += width;
        out.push_str(&unit);
    }
    out
}

#[cfg(test)]
#[path = "tests_unreadable_msg.rs"]
mod tests;
