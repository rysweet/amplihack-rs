use super::{is_amplihack_statusline, is_home_statusline};

#[test]
fn the_home_command_in_each_form_is_amplihacks() {
    for c in [
        "amplihack profile statusline",
        "  amplihack profile statusline  ",
        "'/home/u/.local/bin/amplihack' profile statusline",
        r"'/home/o'\''b/amplihack' profile statusline",
    ] {
        assert!(is_home_statusline(c), "{c}");
        assert!(is_amplihack_statusline(c), "{c}");
    }
}

#[test]
fn the_legacy_script_is_amplihacks_but_not_the_home_command() {
    for c in [
        "~/.amplihack/.claude/tools/statusline.sh",
        "$HOME/.amplihack/.claude/tools/statusline.sh",
        "'/home/u/.amplihack/.claude/tools/statusline.sh'",
    ] {
        assert!(is_amplihack_statusline(c), "{c}");
        assert!(!is_home_statusline(c), "{c}");
    }
}

#[test]
fn a_command_that_does_more_or_only_shares_the_words_is_the_users() {
    for c in [
        "",
        "my-profile statusline-helper.sh",
        "amplihack profile statusline | my-filter",
        "'/x/amplihack' profile statusline | my-filter",
        "'/x/amplihack' profile statusline --owner 'work'",
        "'/x/other-tool' profile statusline",
        "'relative/amplihack' profile statusline",
        "bash ~/.amplihack/.claude/tools/statusline.sh | my-filter",
    ] {
        assert!(!is_amplihack_statusline(c), "{c}");
    }
}
