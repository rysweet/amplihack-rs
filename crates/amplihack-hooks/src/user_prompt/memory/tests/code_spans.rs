//! Foreign text inside code spans and fenced blocks of a memory.

use super::*;

/// Foreign text in a code span is judged like prose: pasting a German
/// error in backticks or a fence doesn't make its words English.
#[test]
fn foreign_text_in_code_spans_is_judged() {
    for (prompt, unrelated) in [
        (
            "/fix the error `ICH BIN NICHT SICHER WARUM DIE TESTS HEUTE SCHEITERN`",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/fix this build error:\n```\nFehler: die Pipeline hat keinen Erfolg, man sieht nichts\n```",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix the bin directory so the workers don't die",
            "Agent general: user: the note says `ich bin nicht sicher warum die tests scheitern`",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    // `~~~` fences and double-backtick spans are code too (CommonMark).
    let man_memory = [memory(
        "Agent x: user: the man with the red hat waved at us from the bus",
    )];
    for prompt in [
        "/fix this:\n~~~\nFehler: die Datei hat man nicht gefunden\n~~~",
        "/fix the ``die Pipeline hat keinen Erfolg man`` error",
        "/fix this:\n```\nFehler: die Datei hat man nicht gefunden\n~~~\nx\n~~~\n```",
        "/fix this:\n```\nFehler: die Datei hat man nicht gefunden\n````",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &man_memory),
            None,
            "{prompt:?} is not about the man in the hat"
        );
    }
    let prompt = "/fix the man who lost his hat on the bus";
    assert_eq!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(
                "Agent x: assistant: The deploy failed with this error:\n~~~\nFehler: die Datei hat man nicht gefunden, der Server ist weg\n~~~"
            )]
        ),
        None
    );
    assert_eq!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(
                "Agent x: assistant: The docs build failed on this page:\n```markdown\nFehler: die Datei hat man nicht gefunden, der Server ist weg\n~~~\nx\n~~~\n```"
            )]
        ),
        None
    );
    // Fences inside block quotes and list items are not recognised, so
    // their text is prose, and a role-label line inside a fence splits
    // the turn; neither makes a German sentence match English words.
    for prompt in [
        "/fix this:\n> ~~~\n> Fehler: die Datei hat man nicht gefunden\n> ~~~",
        "/fix this:\n- ~~~\n  Fehler: die Datei hat man nicht gefunden\n  ~~~",
        "/fix this:\n1. ~~~\n   Fehler: die Datei hat man nicht gefunden\n   ~~~",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &man_memory),
            None,
            "{prompt:?} is not about the man in the hat"
        );
    }
    for unrelated in [
        "Agent x: assistant: The docs build failed on this page:\n> ~~~\n> Fehler: die Datei hat man nicht gefunden, der Server ist weg\n> ~~~",
        "Agent x: assistant: The docs build failed with this log:\n```\nFehler: die Datei hat man nicht gefunden\n\nuser: der Server ist weg, hat man\n```",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    // A paragraph-opening role label starts a turn even inside a code
    // block: a flattened transcript can't tell a pasted log line from a
    // real turn, and splitting never merges two turns' languages.
    assert_eq!(
        turns("assistant: log:\n```\nx\n\nuser: y\n```\n\nuser: next"),
        ["log:\n```\nx", "y\n```", "next"]
    );
    let unrelated = "Agent x: assistant: The build is fixed now and all the tests are green again with the new config. Here is the chat log that you asked for:\n```\nder Mann mit dem Hut hat den Bus verpasst, sagt man\n\nuser: ```\ncargo test\n```\n```";
    assert_eq!(
        format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
        None
    );
    // A stray fence line in one message doesn't pair with the next
    // message's fence to merge a German turn into an English one.
    for unrelated in [
        "Agent x: user: der Mann mit dem Hut hat den Bus verpasst, sagt man. Hier ist mein Code:\n```\nfn main()\n\nassistant: ```\nfn main() {}\n```\nThe build is fixed now and all the tests are green again with the new config.",
        "Agent x: user: der Mann mit dem Hut hat den Bus verpasst, sagt man\n```\n\nassistant: ```rust\nfn main() {}\n```\nThe build is fixed now and all the tests are green again with the new config.",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    assert!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory("Agent x: user: der Mann mit dem Hut hat den Bus verpasst, sagt man. Hier ist mein Code:\n```\nfn main()\n\nassistant: ```\nfn main() {}\n```\nThe man on the bus lost his hat, and the fix is to look for it at the bus depot.")]
        )
        .is_some()
    );
    // A German log pasted into an English turn, with a role label
    // inside it, doesn't make the memory relevant.
    let sqlite_prompt = "/fix the sqlite timeout on the release branch of the payments service";
    assert_eq!(
        format_agent_memory_context(
            sqlite_prompt,
            &prompt_agents(sqlite_prompt),
            &[memory(
                "Agent x: assistant: The sqlite timeout is gone and it was the lock that we saw before. Here is the log:\n```\nder Mann mit dem Hut hat den Bus heute morgen wieder verpasst, weil er seinen Schirm suchte und dabei ganz vergessen hatte\n\nuser: fixed\n```\nkafka parquet grafana vault terraform helm redis nginx prometheus cron ingest webhook oauth"
            )]
        ),
        None
    );
    // A message that starts with a fence still ends where it ends.
    assert_eq!(
        turns("user: ```\nx\n```\nhallo\n\nassistant: ok\n```\ny\n```"),
        ["```\nx\n```\nhallo", "ok\n```\ny\n```"]
    );
    let unrelated = "Agent x: user: ```\ncargo test\n```\nder Mann mit dem Hut hat den Bus verpasst, sagt man, und er ist sehr traurig\n\nassistant: The build is fixed now and the tests pass.\n```\ncargo test\n```";
    assert_eq!(
        format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
        None
    );
    // Inside a fence, a diff, doc-comment or quote line with a fence run
    // after its marker is content, not the fence's end.
    for fenced_prompt in [
        "/fix the markdown diff for the docs page:\n```diff\n- ```\n+ ~~~\n Fehler: die Datei hat man nicht gefunden, der Server ist weg\n```",
        "/fix the jsdoc example for this helper:\n```js\n/**\n * ```\n * Fehler: die Datei hat man nicht gefunden, der Server ist weg\n */\n```",
    ] {
        assert_eq!(
            format_agent_memory_context(fenced_prompt, &prompt_agents(fenced_prompt), &man_memory),
            None,
            "{fenced_prompt:?} is not about the man in the hat"
        );
    }
    for unrelated in [
        "Agent x: assistant: I changed the docs fence for you:\n```diff\n- ```\n+ ~~~\n Fehler: die Datei hat man nicht gefunden, der Server ist weg\n```",
        "Agent x: assistant: The quoted example in the docs:\n```markdown\n> ```\n> x\nFehler: die Datei hat man nicht gefunden, der Server ist weg\n```",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    assert_eq!(
        segments("```diff\n- ```\n+ ~~~\n Fehler hat man\n```"),
        [
            (false, ""),
            (true, "- ```\n+ ~~~\n Fehler hat man\n"),
            (false, "")
        ]
    );
    // A foreign prompt with a pasted foreign error matches no English
    // memory on the words the two languages share.
    for (prompt, unrelated) in [
        (
            "/fix ```\nFehler: die Pipeline hat keinen Erfolg, man sieht nichts\n```\nHat man Ideen?",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix `Fehler: die Pipeline hat keinen Erfolg, man sieht nichts` hat man Ideen?",
            "Agent general: user: the man with the red hat waved at us from the bus",
        ),
        (
            "/fix `ich bin nicht sicher warum die tests scheitern` bin die",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    // With no prose outside code, a pasted English error is what the
    // language is judged on; a pasted German one still fails.
    let pool = [memory(
        "Agent builder: the database connection pool is exhausted when the workers restart",
    )];
    for prompt in [
        "/fix\n```\nerror: the connection to the database was refused because the pool is exhausted\n```",
        "/fix `the database connection pool is exhausted`",
        "/fix this:\n```\nerror: the connection to the database was refused because the pool is exhausted\n```",
    ] {
        assert!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &pool).is_some(),
            "{prompt:?} reads as English"
        );
    }
    let prompt = "/fix the database pool exhaustion when workers restart";
    assert!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory("Agent builder: assistant: ```\nthe database connection pool is exhausted when the workers restart\n```")]
        )
        .is_some()
    );
    assert!(!reads_as_english(
        "```\nFehler: die Pipeline hat keinen Erfolg, man sieht nichts\n```"
    ));
    assert!(!reads_as_english("`src/die/bin.rs` `mit_hat` `was_ist`"));
    // A turn that is only a foreign fence contributes nothing, its
    // identifiers included; with English prose before it, they count.
    let prompt = "/fix the `needless_borrow` lint in `session_stop`";
    assert_eq!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(
                "Agent x: assistant: ```\nwarnung: needless_borrow in session_stop, die Pipeline hat keinen Erfolg\n```"
            )]
        ),
        None
    );
    assert!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory("Agent x: assistant: see this:\n```\nwarnung: needless_borrow in session_stop, die Pipeline hat keinen Erfolg\n```")]
        )
        .is_some()
    );
    // Identifiers in code still match, in short spans and in a
    // non-English block alike.
    for (prompt, relevant) in [
        (
            "/fix the `needless_borrow` lint in `session_stop`",
            "Agent builder: the needless_borrow lint fires in session_stop on every build",
        ),
        (
            "/fix the needless_borrow lint in session_stop",
            "Agent builder: the lint output was:\n```\nwarnung: needless_borrow in session_stop hier und dort\n```",
        ),
    ] {
        assert!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)])
                .is_some(),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
}
