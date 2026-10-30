//! Issue #1733: the shell tokeniser shared by the merge guard and the tmux
//! guard. `commands()` keeps its merge-guard behaviour; `parse()` adds the
//! heredoc bodies each command receives and the pipeline it belongs to.

use super::*;

fn words(text: &str) -> Vec<Vec<String>> {
    parse(text).into_iter().map(|c| c.words).collect()
}

fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn commands_is_parse_without_the_extra_fields() {
    for text in [
        "gh pr merge 38 --squash",
        "cd repo && gh pr merge 38",
        "gh pr view 38; gh pr merge 38\nls -la",
        "cat <<'EOF'\nprose\nEOF\ngh pr merge 38",
        "echo \"$(git log --oneline | head -1)\" | tee out",
        "a | b || c && d & e",
        "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server",
    ] {
        assert_eq!(commands(text), words(text), "differs for {text:?}");
    }
}

#[test]
fn splits_on_every_separator() {
    assert_eq!(
        words("a 1; b 2 && c || d | e & f\ng"),
        vec![
            owned(&["a", "1"]),
            owned(&["b", "2"]),
            owned(&["c"]),
            owned(&["d"]),
            owned(&["e"]),
            owned(&["f"]),
            owned(&["g"]),
        ]
    );
}

#[test]
fn joins_line_continuations_and_removes_quotes() {
    assert_eq!(
        words("tmux 'kill-'\\\n\"server\"\nls"),
        vec![owned(&["tmux", "kill-server"]), owned(&["ls"])]
    );
}

/// Inside double quotes bash removes a backslash only before `$`, a
/// backtick, `"`, `\` or a newline. Before anything else it stays, so
/// `printf "ls\nkill-server"` still holds the `\n` that printf turns into a
/// line break (crusty review of #1735, concern 7).
#[test]
fn double_quotes_keep_a_backslash_before_an_ordinary_character() {
    assert_eq!(
        words(r#"printf "a\nb" "c\$d" "e\"f" "g\\h" "i\`j" "k\tl""#),
        vec![owned(&[
            "printf", "a\\nb", "c$d", "e\"f", "g\\h", "i`j", "k\\tl"
        ])]
    );
}

#[test]
fn a_lone_pipe_keeps_the_pipeline_and_other_separators_start_a_new_one() {
    let cmds = parse("a | b | c; d && e || f & g\nh");
    let p: Vec<usize> = cmds.iter().map(|c| c.pipeline).collect();
    assert_eq!(p.len(), 8, "{:?}", words("a | b | c; d && e || f & g\nh"));
    assert_eq!(p[0], p[1], "a | b");
    assert_eq!(p[1], p[2], "b | c");
    for (i, pair) in p.windows(2).enumerate().skip(2) {
        assert_ne!(pair[0], pair[1], "separator {i} must start a new pipeline");
    }

    // POSIX `pipe_sequence '|' linebreak command`: a newline after a lone
    // `|` or `|&`, with or without a comment or blank lines before it, does
    // not end the pipeline (crusty review of #1735, round 4).
    for text in [
        "a |\nb",
        "a |&\nb",
        "a | # c\nb",
        "a |\n\n  # c\n\nb",
        "a | \\\n\nb",
        "a\\\n| b",
    ] {
        let cmds = parse(text);
        assert_eq!(cmds.len(), 2, "{text:?}: {:?}", words(text));
        assert_eq!(cmds[0].words, owned(&["a"]), "{text:?}");
        assert_eq!(cmds[1].words, owned(&["b"]), "{text:?}");
        assert_eq!(cmds[0].pipeline, cmds[1].pipeline, "{text:?}");
    }
    // Any other separator before the newline still starts a new pipeline.
    for text in ["a ||\nb", "a &&\nb", "a;\nb", "a &\nb", "a\nb", "a # |\nb"] {
        let cmds = parse(text);
        assert_eq!(cmds.len(), 2, "{text:?}: {:?}", words(text));
        assert_ne!(cmds[0].pipeline, cmds[1].pipeline, "{text:?}");
    }
}

/// A heredoc whose `<<` comes before a trailing `|` has its body read at the
/// newline, and the command after the body is still the next stage.
#[test]
fn a_heredoc_before_a_trailing_pipe_keeps_the_pipeline() {
    let cmds = parse("cat <<EOF |\nbody\nEOF\nsh");
    assert_eq!(cmds.len(), 2, "{:?}", words("cat <<EOF |\nbody\nEOF\nsh"));
    assert_eq!(cmds[0].words, owned(&["cat"]));
    assert_eq!(cmds[0].heredocs.len(), 1);
    assert!(cmds[0].heredocs[0].contains("body"));
    assert_eq!(cmds[1].words, owned(&["sh"]));
    assert!(cmds[1].heredocs.is_empty());
    assert_eq!(cmds[0].pipeline, cmds[1].pipeline);
}

/// A stage made only of a redirection or a heredoc is a command, so the
/// newline after it ends the pipeline, as in bash. (A stage with only a
/// redirection has no words and is not kept in the output.)
#[test]
fn a_redirection_only_stage_ends_the_pipeline_at_a_newline() {
    let cmds = parse("a | >out\nb");
    assert_eq!(cmds.len(), 2, "{:?}", words("a | >out\nb"));
    assert_eq!(cmds[0].words, owned(&["a"]));
    assert_eq!(cmds[1].words, owned(&["b"]));
    assert_ne!(cmds[0].pipeline, cmds[1].pipeline);

    let cmds = parse("a | <<EOF\nbody\nEOF\nb");
    assert_eq!(cmds.len(), 3, "{:?}", words("a | <<EOF\nbody\nEOF\nb"));
    assert_eq!(cmds[1].heredocs.len(), 1);
    assert!(cmds[1].heredocs[0].contains("body"));
    assert_eq!(cmds[0].pipeline, cmds[1].pipeline);
    assert_eq!(cmds[2].words, owned(&["b"]));
    assert_ne!(cmds[1].pipeline, cmds[2].pipeline);
}

/// A backslash-newline is removed before words are split, so it adds no
/// empty word between the words around it.
#[test]
fn a_line_continuation_between_words_adds_no_word() {
    assert_eq!(
        words("sudo \\\n  tmux \\\n  kill-server"),
        vec![owned(&["sudo", "tmux", "kill-server"])]
    );
}

#[test]
fn a_double_pipe_is_not_a_pipe() {
    let cmds = parse("a || b");
    assert_eq!(cmds.len(), 2);
    assert_ne!(cmds[0].pipeline, cmds[1].pipeline);
}

#[test]
fn heredoc_body_belongs_to_the_command_that_has_the_redirect() {
    let cmds = parse("cat <<EOF > notes.md\nrun tmux kill-server here\nEOF\nls");
    assert_eq!(cmds.len(), 2, "{:?}", words("cat <<EOF > notes.md\n…"));
    assert_eq!(cmds[0].words.first().map(String::as_str), Some("cat"));
    assert_eq!(cmds[0].heredocs.len(), 1);
    assert!(cmds[0].heredocs[0].contains("run tmux kill-server here"));
    assert_eq!(cmds[1].words, owned(&["ls"]));
    assert!(cmds[1].heredocs.is_empty());
}

#[test]
fn heredoc_body_of_a_shell_is_recorded_on_the_shell() {
    let cmds = parse("bash <<'EOF'\ntmux kill-server\nEOF");
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].words, owned(&["bash"]));
    assert_eq!(cmds[0].heredocs.len(), 1);
    assert!(cmds[0].heredocs[0].contains("tmux kill-server"));
}

#[test]
fn two_heredocs_on_one_line_go_to_their_own_commands() {
    let cmds = parse("cat <<A; bash <<-B\na-body\nA\n\tb-body\n\tB\necho done");
    assert_eq!(cmds.len(), 3);
    assert_eq!(cmds[0].words.first().map(String::as_str), Some("cat"));
    assert_eq!(cmds[0].heredocs.len(), 1);
    assert!(cmds[0].heredocs[0].contains("a-body"));
    assert!(!cmds[0].heredocs[0].contains("b-body"));
    assert_eq!(cmds[1].words.first().map(String::as_str), Some("bash"));
    assert_eq!(cmds[1].heredocs.len(), 1);
    assert!(cmds[1].heredocs[0].contains("b-body"));
    assert!(!cmds[1].heredocs[0].contains("a-body"));
    assert_eq!(cmds[2].words, owned(&["echo", "done"]));
}

#[test]
fn heredoc_into_a_pipeline_shares_the_pipeline_with_the_shell() {
    let cmds = parse("cat <<'EOF' | sh\ntmux kill-server\nEOF");
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].words.first().map(String::as_str), Some("cat"));
    assert!(cmds[0].heredocs[0].contains("tmux kill-server"));
    assert_eq!(cmds[1].words, owned(&["sh"]));
    assert_eq!(cmds[0].pipeline, cmds[1].pipeline);
}

#[test]
fn heredoc_bodies_are_never_words() {
    assert_eq!(
        commands("cat <<EOF\ngh pr merge 38\ntmux kill-server\nEOF"),
        vec![owned(&["cat"])]
    );
}

#[test]
fn a_here_string_is_an_ordinary_word_not_a_heredoc() {
    let cmds = parse("bash <<< 'tmux kill-server'");
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].heredocs.is_empty());
    assert!(cmds[0].words.iter().any(|w| w == "tmux kill-server"));
}

/// 1 000 nested `$(` must not overflow a 2 MiB stack: `subst_end` is bounded
/// or iterative.
#[test]
fn deep_command_substitution_does_not_overflow_a_small_stack() {
    let inputs = [
        format!("echo {}x{}", "$(".repeat(1000), ")".repeat(1000)),
        format!("echo {}x", "$(".repeat(1000)),
        format!("echo \"{}x", "$(".repeat(1000)),
        format!("echo \"{}\"", "$(\"".repeat(1000)),
    ];
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            for text in &inputs {
                let _ = parse(text);
                let _ = commands(text);
            }
        })
        .expect("spawn");
    assert!(handle.join().is_ok(), "parse overflowed or panicked");
}

#[test]
fn malformed_fragments_never_panic() {
    for text in [
        "",
        " ",
        "\n",
        "<<",
        "<<-",
        "<<<",
        "a <<",
        "a <<-",
        "$",
        "$'",
        "$'\\",
        "$(",
        "$((",
        "\\",
        "`",
        "'",
        "\"",
        "\"$(",
        "#",
        "a #",
        ";;",
        "&&",
        "||",
        "|",
        "(",
        ")",
        "{",
        "}",
        "a\\\n",
        "cat <<EOF",
        "cat <<EOF\n",
        "cat <<''\n\n",
        "é\u{0}ü$(é",
        ">",
        "2>&",
        "&>",
        "<>",
        ">|",
        "$'\\x",
        "$'\\u",
        "$'\\c",
        "$'\\777",
    ] {
        let _ = parse(text);
        let _ = commands(text);
    }
}

fn redirects(text: &str) -> Vec<Vec<String>> {
    parse(text).into_iter().map(|c| c.redirects).collect()
}

#[test]
fn redirections_and_their_targets_are_never_words() {
    for (text, expect_words, expect_redirects) in [
        (
            "tmux 2>/dev/null kill-server",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux >/dev/null kill-server",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux > /dev/null kill-server",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux </dev/null kill-server",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux 2>&1 kill-server",
            vec!["tmux", "kill-server"],
            vec!["1"],
        ),
        (
            "tmux &>/dev/null kill-server",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux &>>log kill-server",
            vec!["tmux", "kill-server"],
            vec!["log"],
        ),
        (
            "tmux kill-server>/dev/null",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "tmux kill-server</dev/null",
            vec!["tmux", "kill-server"],
            vec!["/dev/null"],
        ),
        (
            "a >>log >|x <>y 2>&- b",
            vec!["a", "b"],
            vec!["log", "x", "y", "-"],
        ),
        (
            "a >'quoted target' b",
            vec!["a", "b"],
            vec!["quoted target"],
        ),
        ("a a2>x", vec!["a", "a2"], vec!["x"]),
    ] {
        assert_eq!(words(text), vec![owned(&expect_words)], "words of {text:?}");
        assert_eq!(
            redirects(text),
            vec![owned(&expect_redirects)],
            "redirects of {text:?}"
        );
        assert!(tokenise(text).complete, "{text:?} is complete");
    }
}

#[test]
fn the_ampersand_of_a_redirection_does_not_end_the_command() {
    let cmds = parse("tmux ls >/dev/null 2>&1 && echo ok & wait");
    assert_eq!(
        cmds.iter().map(|c| c.words.clone()).collect::<Vec<_>>(),
        vec![
            owned(&["tmux", "ls"]),
            owned(&["echo", "ok"]),
            owned(&["wait"])
        ]
    );
}

#[test]
fn quoted_operators_are_text_and_process_substitution_is_unchanged() {
    assert_eq!(
        words("git commit -m \"a > b; c\" 'x<y'"),
        vec![owned(&["git", "commit", "-m", "a > b; c", "x<y"])]
    );
    assert_eq!(
        words("diff <(tmux ls) <(echo)"),
        vec![
            owned(&["diff", "<"]),
            owned(&["tmux", "ls"]),
            owned(&["<"]),
            owned(&["echo"])
        ]
    );
    assert_eq!(
        words("bash <<< \"tmux ls\""),
        vec![owned(&["bash", "tmux ls"])]
    );
}

#[test]
fn a_redirection_with_no_target_is_incomplete() {
    for text in [
        "a >", "a 2>&", "a > ; b", "a > | b", "a >\nb", "a &>", "a > > x",
    ] {
        assert!(!tokenise(text).complete, "{text:?} should be incomplete");
    }
}

fn decode(text: &str) -> String {
    ansi_c_decode(&text.chars().collect::<Vec<_>>())
}

#[test]
fn ansi_c_escapes_decode_as_bash_does() {
    for (body, expect) in [
        (r"\x6bill-server", "kill-server"),
        (r"kill-\x73erver", "kill-server"),
        (r"\153ill-server", "kill-server"),
        (r"\u006bill", "kill"),
        (r"\U0000006bill", "kill"),
        (r"\x6g", "\u{6}g"),
        (r"\0", "\0"),
        (
            r"\a\b\e\E\f\n\r\t\v",
            "\u{7}\u{8}\u{1b}\u{1b}\u{c}\n\r\t\u{b}",
        ),
        (r#"\\\'\"\?"#, "\\'\"?"),
        (r"\cA\c[", "\u{1}\u{1b}"),
        (r"\q\x\u\c", r"\q\x\u\c"),
        (r"trailing\", r"trailing\"),
        (r"\UFFFFFFFF\ud800", "\u{fffd}\u{fffd}"),
    ] {
        assert_eq!(decode(body), expect, "decoding {body:?}");
    }
}

#[test]
fn ansi_c_words_are_decoded_by_the_tokeniser() {
    assert_eq!(
        words(r"tmux $'\x6bill-server' $'kill-\x73erver' $'\153ill-server' $'it\'s'"),
        vec![owned(&[
            "tmux",
            "kill-server",
            "kill-server",
            "kill-server",
            "it's"
        ])]
    );
}
