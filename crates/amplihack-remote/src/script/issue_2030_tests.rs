//! Issue #2030, decision D10: fleet and remote launches already pass the
//! prompt by `--prompt-file`. These pin that: a 0600 `mktemp` file, removal on
//! exit, and no prompt variable on any command line.

use super::{build_remote_script, build_tmux_script};

fn assert_no_prompt_argv(script: &str) {
    assert!(script.contains("umask 077; mktemp"), "{script}");
    assert!(script.contains("--prompt-file"), "{script}");
    for bad in [
        "-p \"$",
        "--print \"$",
        "$PROMPT\"",
        "$(cat \"$PROMPT_FILE\")",
    ] {
        assert!(!script.contains(bad), "{bad:?} in {script}");
    }
}

#[test]
fn remote_script_uses_private_prompt_file_with_exit_trap() {
    let s = build_remote_script("~/workspace", "auto", 3).unwrap();
    assert_no_prompt_argv(&s);
    assert!(s.contains("trap 'rm -f -- \"$PROMPT_FILE\"' EXIT"), "{s}");
}

#[test]
fn tmux_script_uses_private_prompt_file_and_removes_it() {
    let s = build_tmux_script("sess-1", "auto", 3).unwrap();
    assert_no_prompt_argv(&s);
    assert!(s.contains("rm -f -- '$PROMPT_FILE'"), "{s}");
}
