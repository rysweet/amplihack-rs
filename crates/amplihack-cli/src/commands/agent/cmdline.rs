//! Launch options read from a command line (R3.2, design 4.5): Claude's own
//! options from a `claude` process, and amplihack's flags from an
//! `amplihack claude` ancestor. Only exact option tokens are matched, so a
//! prompt or any other option's value is never interpreted. A value that
//! fails its rule is dropped with a warning that gives only its length.

use super::validate;

/// The Claude options an entry records, plus the two that decide whether a
/// session may be recorded at all (`--fork-session` B3, `-p` B2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeOpts {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub skip_permissions: bool,
    pub add_dirs: Vec<String>,
    pub resume: Option<String>,
    pub fork_session: bool,
    pub print: bool,
}

/// The scalar options and their canonical flag names.
const SCALARS: [&str; 3] = ["--model", "--effort", "--permission-mode"];

/// Parses a `claude` command line (`argv[0]` is the program). Returns the
/// options and warnings for dropped values, without the `warning: ` prefix.
/// Parsing stops at a bare `--`; a later scalar value wins.
pub fn parse_claude(argv: &[String]) -> (ClaudeOpts, Vec<String>) {
    let mut o = ClaudeOpts::default();
    let mut warnings = Vec::new();
    let args = argv.get(1..).unwrap_or(&[]);
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--" {
            break;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v)),
            _ => (arg, None),
        };
        match flag {
            "--dangerously-skip-permissions" if inline.is_none() => o.skip_permissions = true,
            "--fork-session" if inline.is_none() => o.fork_session = true,
            "-p" | "--print" if inline.is_none() => o.print = true,
            "--add-dir" => match inline {
                Some(v) => keep_dir(v, &mut o.add_dirs, &mut warnings),
                None => {
                    while i < args.len() && !args[i].starts_with('-') {
                        keep_dir(&args[i], &mut o.add_dirs, &mut warnings);
                        i += 1;
                    }
                }
            },
            "--resume" | "-r" => {
                let value = match inline {
                    Some(v) => Some(v),
                    None if i < args.len() && !args[i].starts_with('-') => {
                        i += 1;
                        Some(args[i - 1].as_str())
                    }
                    None => None,
                };
                if let Some(v) = value {
                    o.resume = checked("--resume", v, validate::conversation_id, &mut warnings);
                }
            }
            f if SCALARS.contains(&f) => {
                let value = match inline {
                    Some(v) => Some(v),
                    None if i < args.len() => {
                        i += 1;
                        Some(args[i - 1].as_str())
                    }
                    None => None,
                };
                let Some(v) = value else { continue };
                let slot = match f {
                    "--model" => &mut o.model,
                    "--effort" => &mut o.effort,
                    _ => &mut o.permission_mode,
                };
                *slot = checked(f, v, validate::option_value, &mut warnings);
            }
            _ => {}
        }
    }
    (o, warnings)
}

/// `value` when it passes `rule`; otherwise `None` and a length-only warning.
fn checked(
    flag: &str,
    value: &str,
    rule: fn(&str) -> Result<(), String>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match rule(value) {
        Ok(()) => Some(value.to_string()),
        Err(why) => {
            warnings.push(dropped(flag, value, &why));
            None
        }
    }
}

fn dropped(flag: &str, value: &str, rule: &str) -> String {
    format!(
        "ignored {flag} from the claude command line ({} bytes): {rule}",
        value.len()
    )
}

/// An `--add-dir` value is kept unless it holds a control character; it is
/// made absolute against the launch directory when the entry is built.
fn keep_dir(v: &str, dirs: &mut Vec<String>, warnings: &mut Vec<String>) {
    if v.is_empty() || v.chars().any(char::is_control) {
        let rule = "must be a non-empty path without control characters";
        warnings.push(dropped("--add-dir", v, rule));
    } else {
        dirs.push(v.to_string());
    }
}

/// The conversation a command line resumes, or `None` under
/// `--fork-session`, whose `--resume` id names the parent (B3).
pub fn resume_as_conversation(o: &ClaudeOpts) -> Option<&str> {
    if o.fork_session {
        None
    } else {
        o.resume.as_deref()
    }
}

/// amplihack's own flags from an `amplihack claude` (or `launch`) command
/// line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AmplihackClaude {
    pub profile: Option<String>,
    pub subprocess_safe: bool,
    pub no_reflection: bool,
}

/// Options of `amplihack claude` and `launch` that take a value and are not
/// recorded.
const SKIPPED_VALUES: [&str; 6] = [
    "--checkout-repo",
    "--append",
    "--max-turns",
    // Issue #1811: the capacity governor's flags.
    "--workstream",
    "--governor-class",
    "--governor-wait",
];
/// The same, for options only `amplihack claude` has.
const CLAUDE_VALUES: [&str; 2] = ["--prompt-file", "--supervise"];
/// Flags of both that are not recorded.
const SKIPPED_FLAGS: [&str; 3] = ["--docker", "--auto", "--ui"];
/// Flags only `amplihack launch` has; `claude` hands them to Claude.
const LAUNCH_FLAGS: [&str; 4] = [
    "--resume",
    "--continue-session",
    "--skip-permissions",
    "--skip-update-check",
];

/// `None` unless `argv[0]`'s basename is `amplihack` and `argv[1]` is
/// `claude` or `launch`. Parsing stops at `--`, or at the first argument
/// that is not one of that subcommand's options (clap's trailing
/// arguments). A profile that fails its rule is not recorded (S8).
pub fn parse_amplihack_claude(argv: &[String]) -> Option<AmplihackClaude> {
    let program = argv.first()?;
    if program.rsplit('/').next() != Some("amplihack") {
        return None;
    }
    let launch = match argv.get(1).map(String::as_str) {
        Some("claude") => false,
        Some("launch") => true,
        _ => return None,
    };
    let mut a = AmplihackClaude::default();
    let args = &argv[2..];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v)),
            _ => (arg, None),
        };
        let takes_value = flag == "--profile"
            || SKIPPED_VALUES.contains(&flag)
            || (!launch && CLAUDE_VALUES.contains(&flag));
        let is_flag = matches!(flag, "--subprocess-safe" | "--no-reflection")
            || SKIPPED_FLAGS.contains(&flag)
            || (launch && LAUNCH_FLAGS.contains(&flag));
        if !takes_value && !(inline.is_none() && is_flag) {
            break;
        }
        i += 1;
        let value = match (takes_value, inline) {
            (true, Some(v)) => Some(v.to_string()),
            (true, None) => {
                i += 1;
                args.get(i - 1).cloned()
            }
            (false, _) => None,
        };
        match flag {
            "--subprocess-safe" => a.subprocess_safe = true,
            "--no-reflection" => a.no_reflection = true,
            "--profile" => {
                a.profile = value.filter(|v| validate::profile_name(v).is_ok());
            }
            _ => {}
        }
    }
    Some(a)
}
