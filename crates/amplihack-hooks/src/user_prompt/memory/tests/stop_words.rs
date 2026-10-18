//! Stop words: function words of English and other languages, and the
//! developer vocabulary kept as topic words.

use super::*;

/// Two common English function or filler words do not make a chit-chat
/// transcript relevant.
#[test]
fn english_function_words_do_not_make_memories_relevant() {
    for (prompt, unrelated) in [
        (
            "/fix because they said some of it was very broken",
            "Agent general: user: because they are very late, some of them left\n\nassistant: ok",
        ),
        (
            "/analyze why here more than once",
            "Agent general: user: more coffee here than there, once\n\nassistant: ok",
        ),
        (
            "/fix the flaky test, thanks! yes, okay",
            "Agent general: user: thanks, yes okay\n\nassistant: pong",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// Contractions are never topic words, whatever their ending.
#[test]
fn contractions_do_not_make_memories_relevant() {
    for (prompt, unrelated) in [
        (
            "/analyze why doesn't it work, isn't it wired?",
            "Agent analyzer: the cache doesn't expire and isn't cleared",
        ),
        (
            "/analyze why doesn't the build work, don't guess",
            "Agent general: user: it doesn't matter, don't worry\n\nassistant: ok",
        ),
        (
            "/fix I don't know why it isn't working",
            "Agent general: user: I don't like pineapple, it isn't food\n\nassistant: noted",
        ),
        (
            "/fix I'll check why they'll fail",
            "Agent general: user: I'll call you, they'll wait\n\nassistant: ok",
        ),
        (
            "/analyze we'll see if you'd merge it",
            "Agent general: user: we'll have lunch if you'd like\n\nassistant: sure",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    assert_eq!(without_apostrophes("builder's"), Some("builder"));
    assert_eq!(without_apostrophes("Builder\u{2019}S"), Some("Builder"));
    assert_eq!(without_apostrophes("'quoted'"), Some("quoted"));
    assert_eq!(without_apostrophes("s"), Some("s"));
    assert_eq!(without_apostrophes("doesn\u{2019}t"), None);
    assert_eq!(without_apostrophes("we'll"), None);
    assert_eq!(without_apostrophes("you'd"), None);
}

/// Function words of other languages are not topic words: sentences that
/// share only those match nothing, as the same English sentence doesn't.
#[test]
fn other_languages_function_words_do_not_make_memories_relevant() {
    for (prompt, unrelated) in [
        (
            "/fix maybe the build has already failed now",
            "Agent general: user: maybe it has already rained now",
        ),
        (
            "/fix quizás la compilación ya falló ahora",
            "Agent general: user: quizás ya llovió ahora",
        ),
        (
            "/fix vielleicht ist der Build schon jetzt kaputt",
            "Agent general: user: vielleicht regnet es schon jetzt",
        ),
        (
            "/fix peut-être que le build est déjà cassé",
            "Agent general: user: peut-être que la pluie est déjà là",
        ),
        (
            "/fix Kompilierung schlägt fehl weil der Test nicht läuft",
            "Agent general: user: weil der Hund nicht läuft",
        ),
        (
            "/fix сборка падает потому что тест",
            "Agent general: user: потому что погода",
        ),
        (
            "/fix la compilación falla porque el test no funciona",
            "Agent general: user: porque el perro no funciona para nada",
        ),
        (
            "/fix 现在可能已经构建失败",
            "Agent general: user: 现在可能已经下雨",
        ),
        ("/fix 今天的构建问题", "Agent general: user: 今天的天气问题"),
        (
            "/fix 今日も全部ビルドが失敗",
            "Agent general: user: 今日も全部雨",
        ),
        (
            "/fix 今回の場合はビルドが失敗します",
            "Agent general: user: 今回の旅行の場合、パスポートは必要ですか\n\nassistant: はい",
        ),
        (
            "/analyze 本当に最近の問題ですか",
            "Agent general: user: 本当に最近の天気は問題ですね",
        ),
        (
            "/fix 自分の時間の問題",
            "Agent general: user: 自分の時間がない問題",
        ),
        (
            "/fix het build is niet goed",
            "Agent general: user: het weer is niet goed vandaag, het is koud",
        ),
        (
            "/fix to jest tak, nie działa build",
            "Agent general: user: to jest tak, nie wiem czy to dobrze",
        ),
        (
            "/fix jeg tror at build er for langsom",
            "Agent general: user: jeg tror at det er for koldt her i dag",
        ),
        (
            "/fix jag kan inte bygga just nu, det blir fel",
            "Agent general: user: jag kan inte komma just nu, det regnar",
        ),
        (
            "/fix jag vet inte varför bygget inte fungerar just nu",
            "Agent general: user: jag vet inte varför katten inte äter just nu",
        ),
        (
            "/fix most nem megy be a build, hogy van ez",
            "Agent general: user: most nem megy be a vonat, hogy van ez",
        ),
        (
            "/fix most nem tudom hogy mi van a builddel",
            "Agent general: user: most nem tudom hogy mi van az ebéddel",
        ),
        (
            "/fix vi skal have en build som virker, ikke fejler",
            "Agent general: user: vi skal have frokost, ikke kaffe",
        ),
        (
            "/fix ¿qué has hecho con el build? no funciona para nada",
            "Agent general: user: ¿qué has hecho con la cena? no me gusta para nada",
        ),
        (
            "/fix has probado otra vez, porque todavía falla",
            "Agent general: user: has llamado a tu madre otra vez, porque todavía espera",
        ),
        (
            "/fix has vist com falla el build avui",
            "Agent general: user: has vist com plou avui",
        ),
        // Any role label starts a turn, not only `user:` / `assistant:`.
        (
            "/fix jag vet inte varför bygget inte fungerar",
            "Agent general: human: jag kan inte komma, det regnar\n\nassistant: That is a shame, the weather should improve later this week.\n\nhuman: jag vet inte varför katten inte äter",
        ),
        (
            "/fix jag kan inte bygga, det blir fel",
            "Agent general: user: jag kan inte komma, det regnar\n\ntool: The command completed and returned the output",
        ),
        (
            "/fix jag kan inte bygga, det blir fel",
            "Agent general: user: jag kan inte komma, det regnar\n\nsystem: The command completed and returned the output",
        ),
        // A non-English user turn answered by an English assistant turn:
        // the English turn is understood, the user's words are not.
        (
            "/fix por favor, ¿puedes arreglar como falla esta compilación?",
            "Agent general: user: por favor, ¿puedes explicar como funciona esta función para mí?\n\nassistant: Sure, this function parses the config file and returns the settings.",
        ),
        (
            "/fix el build falla, pero no sé porque",
            "Agent general: user: el gato duerme, pero no sé porque\n\nassistant: Cats sleep a lot because it saves their energy.",
        ),
        (
            "/fix der Build ist kaputt, warum nicht",
            "Agent general: user: der Hund ist müde, warum nicht\n\nassistant: Dogs get tired after they have been walking for a long time.",
        ),
        (
            "/fix het build is niet goed",
            "Agent general: user: het weer is niet goed\n\nassistant: That is a shame, the weather should improve later this week.",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// Developer vocabulary that SMART lists (`value`, `name`, `first`,
/// `second`, `last`, `new`, `help`, `example`) and `user`, outside a
/// transcript's `user:` label, are topic words.
#[test]
fn developer_vocabulary_is_matched() {
    for (prompt, relevant) in [
        (
            "/fix the value name of the first test",
            "Agent general: test name value must match the first fixture",
        ),
        (
            "/fix the second index and the last index",
            "Agent general: the second index is off by one and the last index panics",
        ),
        (
            "/analyze why the new help example is unavailable",
            "Agent general: the help example for the new command",
        ),
        (
            "/analyze user login",
            "Agent general: the user login uses oauth",
        ),
    ] {
        let result =
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]);
        assert!(
            result.is_some_and(|text| text.contains(strip_agent_prefix(relevant))),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
}

/// Each foreign function word in the stop list keeps a prompt in its
/// language from matching an English memory that uses the same word as
/// English. Each pair shares one real topic word and two stop-listed
/// ones, so it matches as soon as either of the two is a topic word.
#[test]
fn foreign_function_words_are_stop_words() {
    for (prompt, unrelated) in [
        // German: den, mit, war
        (
            "/fix der Token war mit den Dateien kaputt",
            "Agent general: user: The MIT token for the server is in the den, and the old war logs are gone.",
        ),
        // Dutch: doe, met
        (
            "/fix de token doe het niet met de logs",
            "Agent general: user: The token for the doe tag was met by the server.",
        ),
        // French: car, ton
        (
            "/fix ton token car le serveur refuse",
            "Agent general: user: The token for the car is a ton of bytes, and the server rejects it.",
        ),
        // Spanish and Italian: con, era
        (
            "/fix el token con el servidor que era viejo",
            "Agent general: user: The con of the token is that the server from that era rejects it.",
        ),
        // Spanish: sea, van
        (
            "/fix los logs del token van al archivo, sea cual sea el servidor",
            "Agent general: user: The token is in the van by the sea, and the server rejects it.",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
}

/// Developer words that are also foreign function words (`bin`, `dos`)
/// are topic words: a two-word prompt about them still matches.
#[test]
fn developer_words_are_not_foreign_stop_words() {
    for (prompt, relevant) in [
        (
            "/fix the hooks bin",
            "Agent builder: The hooks bin target is missing from the workspace members, so cargo install skips it.",
        ),
        (
            "/fix the bin directory",
            "Agent general: user: The build copies files into the bin directory, and the workers die if it is missing",
        ),
        (
            "/fix the dos endings",
            "Agent builder: The dos line endings in the install script break the parser on linux.",
        ),
    ] {
        let result =
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(relevant)]);
        assert!(
            result.is_some_and(|text| text.contains(strip_agent_prefix(relevant))),
            "{relevant:?} is relevant to {prompt:?}"
        );
    }
}

/// Shared numbers are not shared topics.
#[test]
fn numbers_are_not_topic_words() {
    let terms = topic_terms(
        "v2 sha256 e2e utf8 x86 500 2026 2nd 10am 100ms",
        &HashSet::new(),
    );
    let mut terms = terms.into_iter().collect::<Vec<_>>();
    terms.sort();
    assert_eq!(terms, ["e2e", "sha256", "utf8", "x86"]);
    for (prompt, unrelated) in [
        (
            "/fix the 500 errors after 100 requests",
            "Agent general: user: I need 500 grams of flour and 100 grams of sugar for the cake",
        ),
        (
            "/fix the retries on the 2nd and 3rd attempt",
            "Agent general: user: we met on the 2nd and 3rd of May at the beach with the kids",
        ),
        (
            "/analyze why the backup job at 10am and 5pm fails",
            "Agent general: user: the dentist is at 10am and the gym is at 5pm so I can't come",
        ),
        (
            "/analyze why the 2026 release fails with 404",
            "Agent general: user: we booked the 2026 trip but the hotel page returned a 404 when we paid",
        ),
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &prompt_agents(prompt), &[memory(unrelated)]),
            None,
            "{unrelated:?} is not relevant to {prompt:?}"
        );
    }
    let prompt = "/fix the sha256 checksum errors after 100 requests";
    assert!(
        format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(
                "Agent general: the sha256 checksum fails after 100 requests to the cache"
            )]
        )
        .is_some()
    );
}

/// A definition reference is how a prompt invokes an agent, not what it
/// is about: a memory stored from a prompt that used the same nested
/// reference shares its directory names (`claude`, `amplihack`, `core`),
/// which must not make it relevant to an unrelated prompt.
#[test]
fn definition_reference_paths_are_not_topic_words() {
    let reference = "@.claude/agents/amplihack/core/builder.md";
    let css = format!(
        "Agent builder: user: use {reference} to tidy the css grid on the landing page\n\nassistant: I tidied the css grid so that the landing page columns line up on mobile."
    );
    let unrelated = format!("{reference} why does the auth middleware reject expired tokens");
    assert_eq!(
        format_agent_memory_context(&unrelated, &agents(&["builder"]), &[memory(&css)]),
        None,
        "{unrelated:?} matched the memory by its reference path"
    );
    for related in [
        format!("{reference} why do the css grid columns break on the landing page"),
        // Too short to judge only once the reference is blanked: its
        // path words would otherwise make it long enough to be judged,
        // and it has no English function words.
        format!("{reference} tidy css grid"),
    ] {
        let result = format_agent_memory_context(&related, &agents(&["builder"]), &[memory(&css)])
            .unwrap_or_else(|| panic!("{related:?} still gets the on-topic memory"));
        assert!(result.contains("tidied the css grid"));
    }

    // Each side is blanked on its own account. The prompt's reference
    // must not match the same words written as prose in a memory ...
    let prose =
        memory("Claude loads the amplihack core bundle from the staged directory at startup.");
    assert_eq!(
        format_agent_memory_context(&unrelated, &agents(&["builder"]), &[prose]),
        None,
        "the prompt's reference path matched prose in the memory"
    );
    // ... and a memory's reference must not match them in the prompt.
    let prose_prompt = "/build why does claude skip the amplihack core hooks";
    assert_eq!(
        format_agent_memory_context(prose_prompt, &agents(&["builder"]), &[memory(&css)]),
        None,
        "the memory's reference path matched prose in the prompt"
    );

    assert_eq!(
        without_definition_references(
            "use @~/.amplihack/.claude/agents/amplihack/core/builder.md to tidy, then read README.md"
        ),
        "use   to tidy, then read README.md"
    );
    assert_eq!(
        without_definition_references("see @/home/me/proj/.claude/agents/x/builder.md now"),
        "see   now"
    );
}
