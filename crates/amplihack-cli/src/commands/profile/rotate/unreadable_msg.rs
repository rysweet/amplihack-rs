//! Issue #2820: the rotate log's line for a process of this user whose
//! environment the liveness scan could not read. Pure formatting; see
//! "Unreadable process lines" in docs/reference/profile-command.md.
//!
//! The arguments are under the control of any process of this user, so the
//! order of work is mask, then escape, then cut: a value [`masked`] hides
//! never shows, a cut never splits an escape, and one process is always
//! exactly one line that cannot forge another. The environment itself is
//! never read, so it cannot reach this text.
//!
//! Masking hides what is shaped like a secret, not every secret. A secret
//! passed as a bare argument, with no secret-named flag before it and no
//! known token prefix, is still shown within the 120 characters. The same
//! user can read it in `/proc/<pid>/cmdline`, and the log is a private file
//! (`log.rs` creates it 0600 and refuses one that others can read).

use crate::commands::profile::auto_switch::{Unreadable, Verdict, assignment_name};

/// What a hidden value is shown as.
const HIDDEN: &str = "<hidden>";
/// A flag or header whose name holds one of these, ignoring case, carries a
/// secret. A word inside a longer name counts (`--api-key`, `X-Auth-Token`);
/// hiding a harmless value by mistake costs only log detail.
const SECRET_WORDS: [&str; 9] = [
    "token",
    "secret",
    "passw",
    "passphrase",
    "key",
    "auth",
    "cred",
    "cookie",
    "bearer",
];
/// Well-known credential prefixes: Anthropic and OpenAI keys, GitHub,
/// GitLab and Slack tokens, AWS access key ids, Google API keys, JWTs, npm
/// and Hugging Face tokens. An argument, or a flag's value, that starts
/// with one is hidden whole.
const TOKEN_PREFIXES: [&str; 19] = [
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ",
    "npm_",
    "hf_",
];

/// The most process lines [`lines`] gives; the rest are counted in one
/// summary line. Every process is still judged: this caps the log only.
pub const MAX_LINES: usize = 32;
/// The most characters of the command line shown, counted after escaping.
const MAX_COMMAND: usize = 120;
/// The most characters of the executable shown, counted after escaping.
const MAX_EXE: usize = 32;

/// `unreadable process <pid> (<exe>) "<command>": <verdict>`.
pub fn line(u: &Unreadable) -> String {
    format!(
        "unreadable process {} ({}) \"{}\": {}",
        u.pid,
        cut(&u.exe, MAX_EXE),
        cut(&masked(&u.argv).join(" "), MAX_COMMAND),
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

/// The arguments as the log shows them. A flag with no `=` whose name holds
/// a [`SECRET_WORDS`] word (`--token`, `--api-key`) hides the argument after
/// it, whatever that is. Every argument is then masked by [`mask`].
fn masked(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut hide_next = false;
    for arg in argv {
        if std::mem::take(&mut hide_next) {
            out.push(HIDDEN.to_string());
            continue;
        }
        hide_next = arg.starts_with('-') && !arg.contains('=') && secret_name(arg);
        out.push(mask(arg));
    }
    out
}

/// One argument with what it carries hidden, by the first rule that fits:
///
/// - `NAME=value` as `NAME=<hidden>`: an environment entry handed on.
/// - `--flag=value` as `--flag=<hidden>` when the flag is secret-named; as
///   `--flag=NAME=<hidden>` when the value is `NAME=value`; otherwise its
///   value as [`mask_value`] gives it.
/// - Anything else as [`mask_value`] gives it.
fn mask(arg: &str) -> String {
    if let Some(name) = assignment_name(arg) {
        return format!("{name}={HIDDEN}");
    }
    if arg.starts_with('-')
        && let Some((flag, rest)) = arg.split_once('=')
    {
        if secret_name(flag) {
            return format!("{flag}={HIDDEN}");
        }
        if let Some(name) = assignment_name(rest) {
            return format!("{flag}={name}={HIDDEN}");
        }
        return format!("{flag}={}", mask_value(rest));
    }
    mask_value(arg)
}

/// A whole argument, or the value of a `--flag=value`, by the first rule
/// that fits:
///
/// - One that starts with a [`TOKEN_PREFIXES`] prefix as `<hidden>`.
/// - `Name: value` as `Name: <hidden>` when `Name` is one word and
///   secret-named: a header such as `Authorization: Bearer ...`.
/// - Otherwise the value with the user and password of every URL in it
///   hidden (`https://<hidden>@host/...`).
///
/// A token inside other text is not found.
fn mask_value(v: &str) -> String {
    if TOKEN_PREFIXES.iter().any(|p| v.starts_with(p)) {
        return HIDDEN.to_string();
    }
    if let Some((name, _)) = v.split_once(':')
        && !name.is_empty()
        && !name.contains(|c: char| c.is_whitespace() || c == '/')
        && secret_name(name)
    {
        return format!("{name}: {HIDDEN}");
    }
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        rest = match tail[..end].rfind('@') {
            Some(at) => {
                out.push_str(HIDDEN);
                &tail[at..]
            }
            None => tail,
        };
    }
    out.push_str(rest);
    out
}

/// Whether a flag or header name holds a [`SECRET_WORDS`] word.
fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| name.contains(w))
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
