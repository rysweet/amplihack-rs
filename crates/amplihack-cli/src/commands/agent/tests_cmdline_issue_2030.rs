//! Issue #2030: the `/proc` argv parsers still read a Claude command line
//! once prompts leave argv — a bare `-p`/`--print`, and the `-file` flags
//! with their path values.

use super::cmdline::*;

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

#[test]
fn bare_print_flags_are_detected() {
    for flag in ["-p", "--print"] {
        let (o, w) = parse_claude(&v(&["claude", "--model", "opus", flag]));
        assert!(o.print, "{flag}");
        assert_eq!(o.model.as_deref(), Some("opus"));
        assert!(w.is_empty(), "{w:?}");
    }
}

#[test]
fn file_flag_values_are_not_read_as_options() {
    let (o, _) = parse_claude(&v(&[
        "claude",
        "--append-system-prompt-file",
        "/home/a/.amplihack/private/system-prompt-append.md",
        "--system-prompt-file",
        "/tmp/amplihack-prompt-x/system-prompt.md",
        "--model",
        "sonnet",
        "-p",
    ]));
    assert!(o.print);
    assert_eq!(o.model.as_deref(), Some("sonnet"));
}

#[test]
fn recipe_wrapper_with_bare_print_is_parsed() {
    let a = parse_amplihack_claude(&v(&[
        "/usr/local/bin/amplihack",
        "claude",
        "--subprocess-safe",
        "--",
        "-p",
        "--system-prompt-file",
        "/tmp/amplihack-prompt-x/system-prompt.md",
    ]))
    .expect("an amplihack claude wrapper");
    assert!(a.subprocess_safe);
}
