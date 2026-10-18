//! Relevance scoring: shared topic words, the threshold and prompt agents.

use super::*;

/// The agents are detected from the prompt exactly as `inject_memory`
/// does, so an agent name the prompt uses is ignored as it is in
/// production, which leaves some prompts a single topic word.
#[test]
fn one_shared_word_is_not_relevant() {
    let pong = [memory(
        "Agent general: user: reply with just: pong\n\nassistant: pong",
    )];
    for prompt in [
        "/improve reply to the reviewer",
        "/fix the reply",
        "/analyze this reply",
    ] {
        let agent_types = prompt_agents(prompt);
        assert!(!agent_types.is_empty(), "{prompt:?} names an agent");
        assert_eq!(
            format_agent_memory_context(prompt, &agent_types, &pong),
            None,
            "nothing is injected for {prompt:?}"
        );
    }
}

/// Known limit: a prompt with one topic word left never matches, even a
/// memory that contains it (see [`MIN_SHARED_TERMS`]).
#[test]
fn single_topic_word_prompt_does_not_match() {
    let prompt = "/fix clippy";
    let result = format_agent_memory_context(
        prompt,
        &prompt_agents(prompt),
        &[memory(
            "Agent fix-agent: run clippy with -D warnings before pushing",
        )],
    );
    assert_eq!(result, None);
}

/// A non-English prompt's function words (`die`, `bin`, `hat`, `mit`)
/// don't match the same English words in an English memory.
#[test]
fn non_english_prompts_are_not_scored() {
    for (prompt, unrelated) in [
        (
            "/fix ich bin nicht sicher, warum die Tests scheitern",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/analyze warum die Pipeline hat keinen Erfolg, man sieht nichts",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix die Tests laufen nicht mit dem neuen Build",
            "Agent general: user: The MIT licence file and the old processes that die at shutdown",
        ),
        // Capitals are words like any other: the prompt is judged.
        (
            "/fix ICH BIN NICHT SICHER, WARUM DIE TESTS SCHEITERN",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/analyze WARUM DIE PIPELINE HAT KEINEN ERFOLG, MAN SIEHT NICHTS",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix la red se cae cuando son las dos",
            "Agent general: user: my son painted the red door for the two of us",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// A command quoted in the prompt, or a memory whose topic words sit
/// only in a fenced or quoted command, is scored on those words: plain
/// words with no English function word are not evidence of another
/// language.
#[test]
fn quoted_commands_keep_their_words() {
    let sqlite = "Agent tester: user: the sqlite test is flaky on linux ci\n\nassistant: The sqlite test times out on the linux runner because the lock is held too long; raising the busy timeout fixed it.";
    for (prompt, relevant) in [
        ("/fix `cargo test sqlite timeout` on ci", sqlite),
        ("/fix `cargo test flaky sqlite timeout` on ci", sqlite),
        ("/fix the `cargo test flaky sqlite timeout`", sqlite),
        (
            "/fix the parquet ingest crash repro",
            "Agent general: user: how do I reproduce it\n\nassistant: Reproduce it with this on the host:\n```\ncargo run ingest parquet crash repro\n```",
        ),
        (
            "/fix the kafka consumer stall repro",
            "Agent general: assistant: Run `kafka consumer stall repro tool` on the host and watch.",
        ),
        (
            "/fix terraform plan vault refresh",
            "Agent general: assistant: Use this:\n```\nterraform plan vault refresh --auto-approve -var=env\n```",
        ),
        // `aux` and `pour` are shell and English words, not evidence.
        (
            "/fix grep ingest worker in ps aux",
            "Agent general: user: how do I find the stuck worker\n\nassistant: Check the stuck worker with:\n```\nps aux | grep ingest worker\n```",
        ),
        (
            "/fix loader rows shard queue flush",
            "Agent general: assistant: The loader runs:\n```\nloader pour rows shard queue flush\n```",
        ),
    ] {
        let result =
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]);
        assert!(
            result.is_some_and(|text| text.contains(&bounded_memory_text(&single_line(
                strip_agent_prefix(relevant)
            )))),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
    // The evidence list holds no English, shell or name words: with
    // `pour` listed, this English fence would reach two hits (`pour` and
    // the accented loanword `café`) and lose its words.
    for word in [
        "aux", "pour", "el", "est", "des", "il", "las", "los", "para", "pas", "uma", "que", "non",
        "com", "ist", "das", "dem", "les", "una", "che", "della", "nella",
    ] {
        assert!(
            !FOREIGN_EVIDENCE.contains(&word),
            "{word:?} is an English or shell word"
        );
    }
    let cafe = "Agent general: assistant: The loader runs:\n```\nloader pour rows into the café shard queue\n```";
    let prompt = "/fix the loader rows for the shard queue";
    assert!(
        format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(cafe)]).is_some(),
        "{cafe:?} is relevant to {prompt:?}"
    );
    // Two acronyms that are also German words are not evidence either:
    // the fence scores the same as with acronyms that are not.
    let prompt = "/fix the export of the array to the ingest bucket";
    let [acronyms, control] = [("IST", "DAS"), ("UTC", "SAN")].map(|(zone, array)| {
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(&format!(
                "Agent general: assistant: The nightly job is:\n```\n02:00 {zone} export of the {array} array to the ingest bucket\n```"
            ))],
        )
        .map(|text| text.split("relevance: ").nth(1).unwrap().to_string())
    });
    assert!(acronyms.is_some(), "the export memory is relevant");
    assert_eq!(acronyms, control, "IST and DAS are not demoted");
    // Evidence is counted per code part: two fences with one hit each
    // both keep their words, where one fence holding both lines would
    // be demoted.
    let none = HashSet::new();
    let two_fences = scored_terms(
        "see:\n```\nFehler: der Spiegel weg\n```\nand\n```\nFehler: Spiegel nicht erreichbar\n```",
        &none,
    );
    for word in ["spiegel", "weg", "erreichbar", "fehler"] {
        assert!(
            two_fences.contains(word),
            "one hit per fence keeps {word:?}: {two_fences:?}"
        );
    }
    let one_fence = scored_terms(
        "see:\n```\nFehler: der Spiegel weg\nFehler: Spiegel nicht erreichbar\n```",
        &none,
    );
    assert!(
        one_fence.is_empty(),
        "both hits in one fence demote it: {one_fence:?}"
    );
    // An accented name in an English fence is not evidence: the memory
    // scores the same as with no name at all (non-ASCII words are never
    // topic words, so the two have the same terms unless one is demoted).
    let prompt = "/fix the parser cleanup rebase from the git log";
    let [umlaut, ascii] = ["Jürgen Müller", ""].map(|author| {
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(&format!(
                "Agent general: assistant: The git log shows:\n```\ncommit 3f2a Author: {author} rebase parser cleanup\n```"
            ))],
        )
        .map(|text| text.split("relevance: ").nth(1).unwrap().to_string())
    });
    assert!(umlaut.is_some(), "the git log memory is relevant");
    assert_eq!(umlaut, ascii, "an accented author name is not demoted");
    // Positive evidence, two hits with a listed word among them, marks
    // a code part as foreign; one hit does not.
    let none = HashSet::new();
    for kept in [
        "see:\n```\ncargo run ingest parquet\n```",
        "see:\n```\nFehler: größere Datei gefunden, cargo run ingest parquet\n```",
        "see `le cargo run ingest parquet` there",
    ] {
        let terms = scored_terms(kept, &none);
        for word in ["cargo", "run", "ingest", "parquet"] {
            assert!(terms.contains(word), "{kept:?} keeps {word:?}: {terms:?}");
        }
    }
    for foreign in [
        "see:\n```\nFehler: die Datei wird nicht gefunden, src/main.rs weg\n```",
        "see:\n```\nFehler: größere Datei nicht gefunden, src/main.rs weg\n```",
        "see `erreur: le fichier src/main.rs manque dans les logs` there",
    ] {
        let mut terms = scored_terms(foreign, &none).into_iter().collect::<Vec<_>>();
        terms.sort();
        assert_eq!(
            terms,
            ["main", "src"],
            "{foreign:?} contributes only its code tokens"
        );
    }
}

/// Foreign text pasted into an English memory's code (a fenced log, a
/// backtick span) is not what the memory is about, so a prompt in that
/// language must not match it on those words. The German words here
/// (`wo`, `ist`, `der`, `nicht`, `grün`) are not in the stop list.
#[test]
fn foreign_prompts_do_not_match_foreign_text_pasted_in_english_memories() {
    let fenced = "Agent general: user: why did the docs build fail\n\nassistant: The docs build failed, here is the log from the runner:\n```\nFehler: die Datei ist nicht gefunden, der Server ist weg, wo ist der Fehler\n```";
    let spanned = "Agent general: assistant: The docs build failed with `Fehler: die Datei ist nicht gefunden, der Server ist weg, wo ist der Fehler` from the runner.";
    for unrelated in [fenced, spanned] {
        for prompt in [
            "/fix wo ist der Test, der ist nicht grün",
            "/fix ich bin nicht sicher warum der Test scheitert",
            "/fix die Datei ist nicht gefunden",
        ] {
            assert_eq!(
                format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
                None,
                "{unrelated:?} is not relevant to {prompt:?}"
            );
        }
    }
    // An English prompt about the memory's own topic still gets it, and
    // the identifiers inside the paste still count.
    for prompt in [
        "/fix the docs build log from the runner",
        "/fix the docs build that failed on the runner",
    ] {
        assert!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(fenced)])
                .is_some(),
            "{fenced:?} is relevant to {prompt:?}"
        );
    }
}

/// The prompt is not language-checked: terse English prompts, nouns and
/// identifiers with no function words, get their memories (#1483 review:
/// a prompt-side check dropped them, and adding an on-topic word such as
/// `linux` could make a memory disappear).
#[test]
fn terse_english_prompts_get_their_memories() {
    let sqlite = "Agent tester: user: the sqlite test is flaky on linux ci\n\nassistant: The sqlite test times out on the linux runner because the lock is held too long; raising the busy timeout fixed it.";
    let clippy = "Agent builder: user: fix the clippy warning in the parser\n\nassistant: I removed the needless_borrow that clippy flagged in the parser.";
    let css = "Agent builder: user: tidy the css grid on the landing page\n\nassistant: I tidied the css grid so that the landing page columns line up on mobile.";
    for (prompt, relevant) in [
        ("/fix flaky sqlite test timeout on linux ci", sqlite),
        ("/fix sqlite flaky test linux", sqlite),
        ("/fix sqlite test flaky", sqlite),
        ("/FIX THE FLAKY SQLITE TEST ON THE LINUX RUNNER", sqlite),
        ("/fix clippy needless_borrow parser", clippy),
        ("/fix clippy needless_borrow in src/parser.rs again", clippy),
        ("/builder tidy css grid landing page", css),
    ] {
        let result =
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]);
        assert!(
            result.is_some_and(|text| text.contains(&single_line(strip_agent_prefix(relevant)))),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
}

/// A path segment that happens to name an agent is not an agent; a slash
/// word is one wherever it stands, the end of the prompt included.
#[test]
fn only_slash_words_name_agents() {
    assert!(
        prompt_agents("the notes under docs/security are stale, and the auth tokens expire")
            .is_empty()
    );
    assert!(
        prompt_agents("move src/database into crates/patterns and rerun the cleanup tests")
            .is_empty()
    );
    assert!(
        prompt_agents("check ~/.amplihack/bin/amplihack-hook and /amplihack-recipe-runner output")
            .is_empty()
    );
    assert_eq!(prompt_agents("please /reflect"), ["reflection"]);
    assert_eq!(prompt_agents("why did CI fail? /fix"), ["fix-agent"]);
    assert_eq!(
        prompt_agents("run /analyzer on /skills and /builder here"),
        ["analyzer", "builder"]
    );
    assert_eq!(
        prompt_agents("/analyze /fix both"),
        ["analyzer", "fix-agent"]
    );
    assert_eq!(prompt_agents("please /reflect."), ["reflection"]);
    assert_eq!(prompt_agents("(/analyze this) now"), ["analyzer"]);
    assert_eq!(
        prompt_agents("run /analyze, then /fix."),
        ["analyzer", "fix-agent"]
    );
    assert!(prompt_agents("see (/bin) and /skills.").is_empty());
    // An explicit definition reference names any agent, project ones too.
    assert_eq!(
        prompt_agents("@.claude/agents/my-reviewer.md review the auth flow"),
        ["my-reviewer"]
    );
    assert_eq!(
        prompt_agents("Use my-reviewer.md agent to review"),
        ["my-reviewer"]
    );
    // An agent name is one file name, never a run of prose up to a
    // later `.md`.
    assert_eq!(
        prompt_agents("@.claude/agents/my-reviewer.md review, then check README.md"),
        ["my-reviewer"]
    );
    assert_eq!(
        prompt_agents("Include @.claude/agents/amplihack/core/builder.md then read docs.md"),
        ["builder"]
    );
    // A definition reference names its agent at any directory depth.
    assert_eq!(
        prompt_agents(
            "@.claude/agents/team/security-reviewer.md why does the auth middleware reject expired tokens"
        ),
        ["security-reviewer"]
    );
    assert_eq!(
        prompt_agents(
            "@.claude/agents/amplihack/guide.md why does the auth middleware reject expired tokens"
        ),
        ["guide"]
    );
    assert_eq!(
        prompt_agents("Include @.claude/agents/team/reviewer.md then read docs.md"),
        ["reviewer"]
    );
    assert_eq!(
        prompt_agents("Include @.claude/agents/a/b/c/deep-agent.md now"),
        ["deep-agent"]
    );
    // A path before `.claude/agents/`: an installed or user-level one.
    assert_eq!(
        prompt_agents(
            "Include @~/.amplihack/.claude/agents/amplihack/core/architect.md -- why does the auth middleware reject expired tokens"
        ),
        ["architect"]
    );
    assert_eq!(
        prompt_agents("@~/.claude/agents/my-reviewer.md review the auth flow"),
        ["my-reviewer"]
    );
    assert_eq!(
        prompt_agents(
            "@/home/me/proj/.claude/agents/amplihack/core/builder.md why does the auth middleware reject expired tokens"
        ),
        ["builder"]
    );
    assert_eq!(
        prompt_agents("@/Users/me/.claude/agents/my-reviewer.md review the auth flow"),
        ["my-reviewer"]
    );
    assert_eq!(
        prompt_agents("/analyze @/home/me x.md and @//.claude/agents/x.md"),
        ["analyzer"]
    );
    // `Use <name>.md agent` takes the same file stems as a reference.
    assert_eq!(prompt_agents("Use agent2.md agent to review"), ["agent2"]);
    assert_eq!(
        prompt_agents("Use security_reviewer.md agent to review"),
        ["security-reviewer"]
    );
    // `Use <name>.md agent` needs its capital `U`.
    assert!(prompt_agents("use my-reviewer.md agent to review").is_empty());
}

#[test]
fn long_transcript_is_not_relevant_just_by_sharing_words() {
    let transcript = (0..400)
        .map(|index| format!("word{index} cargo"))
        .collect::<Vec<_>>()
        .join(" ");
    let result = format_agent_memory_context(
        "/analyze cargo build",
        &agents(&["analyzer"]),
        &[memory(&transcript)],
    );
    assert_eq!(result, None);
}

/// Two shared topic words are necessary but not sufficient: a memory
/// that shares them yet scores below [`RELEVANCE_THRESHOLD`] is dropped.
/// Pins the threshold on its own, apart from [`MIN_SHARED_TERMS`].
#[test]
fn two_shared_words_below_the_threshold_are_not_relevant() {
    let prompt = "/analyze parser cleanup";
    let analyzer = agents(&["analyzer"]);
    let transcript = |fillers: usize| {
        let words = (0..fillers)
            .map(|index| {
                let [high, low] = [index / 26, index % 26].map(|n| char::from(b'a' + n as u8));
                format!("the zz{high}{low}")
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("Agent general: assistant: the parser cleanup touched {words}")
    };
    let prompt_terms = topic_terms(prompt, &ignored_terms(&analyzer));
    for (fillers, relevant) in [(10, true), (80, false)] {
        let text = transcript(fillers);
        let memory_terms = topic_terms(strip_agent_prefix(&text), &ignored_terms(&analyzer));
        let shared = prompt_terms.intersection(&memory_terms).count();
        let relevance = memory_relevance(&prompt_terms, &memory_terms);
        assert_eq!(
            shared, MIN_SHARED_TERMS,
            "{fillers} fillers: {memory_terms:?}"
        );
        assert_eq!(
            relevance >= RELEVANCE_THRESHOLD,
            relevant,
            "{fillers} fillers score {relevance:.3}"
        );
        let result = format_agent_memory_context(prompt, &analyzer, &[memory(&text)]);
        assert_eq!(result.is_some(), relevant, "{fillers} fillers: {result:?}");
    }
}
