//! The English check on memory turns.

use super::*;

/// Sentences that share only grammar — Japanese endings, Korean
/// endings and question words, katakana fragments — match nothing.
#[test]
fn japanese_and_korean_grammar_does_not_make_memories_relevant() {
    for (prompt, unrelated) in [
        (
            "/analyze ビルドが失敗しました",
            "Agent analyzer: デプロイが成功しました",
        ),
        (
            "/analyze サーバーのエラーをチェック",
            "Agent general: パーサーのエラーメッセージ",
        ),
        (
            "/analyze データベースのエラー",
            "Agent general: ユーザーのデータをフィルター",
        ),
        ("/analyze マスターブランチ", "Agent general: スターを付けた"),
        (
            "/analyze コンピューターのモニター",
            "Agent general: データのフィルター",
        ),
        (
            "/analyze 빌드가 왜 실패하는지 어떻게 알 수 있습니까",
            "Agent general: 배포를 어떻게 롤백할 수 있습니까",
        ),
        (
            "/analyze 빌드가 실패했습니다",
            "Agent analyzer: 배포가 성공했습니다",
        ),
        (
            "/fix 테스트가 실패합니다 어떻게 해야 합니까",
            "Agent general: user: 점심은 어떻게 해야 합니까",
        ),
        (
            "/fix 빌드가 실패했는데 왜 그런지 모르겠습니다",
            "Agent general: user: 점심을 먹었는데 왜 그런지 모르겠습니다",
        ),
        (
            "/analyze 테스트가 실패하는 이유가 뭔가요 알려주세요",
            "Agent general: user: 날씨가 좋은 이유가 뭔가요 알려주세요",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// Chinese particles and pronouns are not topic words either.
#[test]
fn chinese_function_characters_do_not_make_memories_relevant() {
    for (prompt, unrelated) in [
        (
            "/analyze 我们的构建失败了吗",
            "Agent analyzer: 我们的部署成功了吗",
        ),
        (
            "/analyze 为什么我们的构建失败了",
            "Agent general: 我们的部署脚本需要更新",
        ),
        (
            "/analyze 这个测试为什么失败",
            "Agent general: 这个部署为什么成功",
        ),
        (
            "/fix 我们的构建失败了",
            "Agent general: user: 我们的午饭吃什么\n\nassistant: 我们的午饭吃面条",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// Known limit: only English is understood, so a memory in another
/// language is never injected, even a relevant one (see [`topic_terms`]).
#[test]
fn non_english_memories_are_not_injected() {
    for (prompt, relevant) in [
        (
            "/analyze 构建失败",
            "Agent analyzer: 构建失败时先运行 cargo fmt",
        ),
        (
            "/analyze ビルドが失敗しました",
            "Agent analyzer: ビルド失敗の原因は cargo fmt でした",
        ),
        (
            "/analyze 빌드 실패",
            "Agent analyzer: 빌드 실패 원인은 포맷",
        ),
        (
            "/fix la compilación falla por cargo fmt",
            "Agent general: la compilación falla por cargo fmt",
        ),
        (
            "/fix der Build schlägt wegen cargo fmt fehl",
            "Agent general: der Build schlägt wegen cargo fmt fehl",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]),
            None,
            "{relevant:?} is not scored"
        );
    }
}

#[test]
fn language_check_counts_english_function_words() {
    assert!(reads_as_english("Fix CI by running cargo fmt before push."));
    assert!(reads_as_english(
        "user: reply with just: pong\n\nassistant: pong"
    ));
    assert!(!reads_as_english("vielleicht regnet es schon jetzt"));
    assert!(!reads_as_english("porque el perro no funciona para nada"));
    assert!(!reads_as_english("cargo fmt"));
    assert!(!reads_as_english(""));
    // With prose outside code, the code's words don't count.
    assert!(!reads_as_english(
        "cargo fmt ```\nlet the = it;\n``` `the` `and`"
    ));
    assert_eq!(
        prose_words("“The build” fails «again» ¡now! „quoted“ ¿qué? 'this'"),
        [
            "the", "build", "fails", "again", "now", "quoted", "qué", "this"
        ]
    );
    assert_eq!(
        prose_words(
            "Fix CI: run `cargo fmt --all` then --test-threads=1 on src/main.rs:12 (DEFAULT_STEP_TIMEOUT), re-ran."
        ),
        ["fix", "ci", "run", "then", "on"]
    );
}

/// Session-stop stores the head of a coding transcript: English prose
/// full of paths, flags and identifiers. The code is not held against
/// the prose.
#[test]
fn technical_english_memories_are_injected() {
    for (prompt, relevant) in [
        (
            "/fix the release workflow token permissions",
            "Agent general: user: release workflow fails with 403\n\nassistant: GITHUB_TOKEN lacks contents: write. Added permissions: contents: write to .github/workflows/release.yml; re-ran, green.",
        ),
        (
            "/fix cargo clippy warnings in the hooks crate",
            "Agent builder: Fix CI: run cargo fmt --all then cargo clippy -D warnings on the hooks crate",
        ),
        (
            "/fix the flaky sqlite test",
            "Agent tester: Flaky sqlite_end_to_end test times out under parallel runs; use --test-threads=1",
        ),
        (
            "/fix the flaky sqlite test",
            "Agent tester: The sqlite test is flaky because it shares a temp dir with other tests; give each test its own tempdir.",
        ),
        (
            "/analyze the recipe runner timeout",
            "Agent general: user: /analyze recipe runner timeout\n\nassistant: Root cause: `amplihack-recipe-runner` kills steps after 300s (DEFAULT_STEP_TIMEOUT in crates/amplihack-recipe/src/runner.rs:88). Long cargo builds exceed it. Fix: raise to 1800s or make it configurable via AMPLIHACK_STEP_TIMEOUT.",
        ),
        (
            "/fix the clippy needless_borrow warning in session_stop",
            "Agent builder: user: fix the clippy warning\n\nassistant: warning: this expression creates a reference which is immediately dereferenced by the compiler\n --> crates/amplihack-hooks/src/session_stop/mod.rs:57:21\n  = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#needless_borrow\nRemoved the needless borrow in session_stop.",
        ),
    ] {
        let result =
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]);
        let body = single_line(strip_agent_prefix(relevant));
        assert!(
            result.is_some_and(|text| text.contains(&bounded_memory_text(&body))),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
}

/// In a mixed transcript the English turn still counts, and an unclosed
/// backtick (a span cut by session-stop's 500-character head) does not
/// hide the prose after it.
#[test]
fn english_turns_and_unclosed_spans_are_read() {
    for (prompt, relevant) in [
        (
            "/fix the config file parser settings",
            "Agent general: user: ¿puedes explicar esta función?\n\nassistant: Sure, this function parses the config file and returns the settings.",
        ),
        (
            "/fix the sqlite flaky test",
            "Agent tester: user: run `cargo test\n\nassistant: The sqlite test is flaky because it shares a temp dir",
        ),
        // A note opening with a lower-case topic word is not a role label.
        (
            "/analyze the sqlite timeout",
            "Agent tester: sqlite: the timeout is too short when it runs in parallel",
        ),
        (
            "/analyze the sqlite timeout",
            "Agent tester: Sqlite: the timeout is too short when it runs in parallel",
        ),
    ] {
        assert!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)])
                .is_some(),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
    assert_eq!(
        turns("user: hola amigo\n\nassistant: hello there"),
        ["hola amigo", "hello there"]
    );
    assert_eq!(
        turns("human: hola\n\nassistant: see:\n```\nfn a() {}\n\nfn b() {}\n```\n\ntool: done"),
        ["hola", "see:\n```\nfn a() {}\nfn b() {}\n```", "done"]
    );
    assert_eq!(turns("Note: plain text"), ["Note: plain text"]);
    assert_eq!(
        turns("sqlite: the timeout\n\nauth: tokens expire"),
        ["sqlite: the timeout\nauth: tokens expire"]
    );
    assert_eq!(
        segments("a `b` c `d"),
        [(false, "a "), (true, "b"), (false, " c `d")]
    );
    assert_eq!(
        segments("a ``x ` y`` b ```\nfn f() {}\n``` c"),
        [
            (false, "a "),
            (true, "x ` y"),
            (false, " b "),
            (true, "\nfn f() {}\n"),
            (false, " c")
        ]
    );
    // Fences nest the CommonMark way: other-character fence lines are
    // content, a closer may be longer, and an info string can't close.
    assert_eq!(
        segments("```\n~~~\n```\nx\n~~~\n"),
        [(false, ""), (true, "~~~\n"), (false, "x\n~~~\n")]
    );
    assert_eq!(
        segments("see:\n~~~\nhallo\n~~~ rust\nwelt\n~~~\n"),
        [
            (false, "see:\n"),
            (true, "hallo\n~~~ rust\nwelt\n"),
            (false, "")
        ]
    );
    assert_eq!(
        segments("```\nhallo\n````\nafter"),
        [(false, ""), (true, "hallo\n"), (false, "after")]
    );
    assert_eq!(
        segments("see:\n~~~ text\nhallo welt\n~~~~\nafter ~~~\nopen"),
        [
            (false, "see:\n"),
            (true, "hallo welt\n"),
            (false, "after ~~~\nopen")
        ]
    );
}

/// Capitals don't hide a prompt's or a memory's words from the language
/// check: the words that are scored are the words that are judged.
#[test]
fn capitalised_foreign_text_is_judged() {
    for (prompt, unrelated) in [
        (
            "/fix this: ICH BIN NICHT SICHER, WARUM DIE TESTS HEUTE SCHEITERN, SAGT JEMAND",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/fix the bin directory so the workers don't die",
            "Agent general: user: the note says ICH BIN NICHT SICHER WARUM DIE TESTS SCHEITERN",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    // Letter case never changes the outcome. (One English word ahead
    // of nine German ones is 1 marker in 10 prose words in either case:
    // a mixed-language prompt, the documented residual tracked in #1500.)
    for (prompt, memory_text) in [
        (
            "/analyze why: WARUM DIE PIPELINE HAT KEINEN ERFOLG, MAN SIEHT NICHTS",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix the error ICH BIN NICHT SICHER, WARUM DIE TESTS SCHEITERN",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/fix this: ICH BIN NICHT SICHER, WARUM DIE TESTS HEUTE SCHEITERN, SAGT JEMAND",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
    ] {
        let memories = [memory(memory_text)];
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &memories),
            format_agent_memory_context(&prompt.to_lowercase(), &prompt_agents(prompt), &memories),
            "{prompt:?} is judged the same in either case"
        );
    }
}

/// Known limit: a note whose prose has too few English function words
/// to tell its language is not scored, even when it is relevant (see
/// [`reads_as_english`]).
#[test]
fn terse_notes_without_english_function_words_are_not_scored() {
    for (prompt, relevant) in [
        (
            "/analyze user login",
            "Agent general: user login uses oauth",
        ),
        (
            "/fix clippy warnings in the hooks crate",
            "Agent builder: clippy warnings in hooks crate: needless_borrow, redundant_clone; fixed via cargo clippy --fix",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]),
            None,
            "{relevant:?} is not scored"
        );
    }
}
