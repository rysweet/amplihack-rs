//! Agent memory injection and framework file detection.

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, slash_commands_for,
};
use amplihack_memory::cli_memory::{PromptContextMemory, retrieve_prompt_context_memories};
use amplihack_types::ProjectDirs;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Minimum relevance a memory needs before it is injected into the prompt.
const RELEVANCE_THRESHOLD: f64 = 0.2;
/// A memory must also share at least this many topic words with the prompt,
/// so one coincidental word never makes a short memory relevant.
///
/// Known limit, chosen deliberately: a prompt left with a single topic word
/// once its agent names and slash command are ignored (`/fix clippy`) never
/// matches. Letting one word suffice re-admits exactly the #1483 failure
/// (`/improve reply to the reviewer` pulling in the stored `reply with just:
/// pong` smoke test), and one shared word is not evidence of relevance.
const MIN_SHARED_TERMS: usize = 2;
/// Topic words shorter than this (`rs`, `md`, `ci`) are too common to count.
const MIN_TERM_CHARS: usize = 3;
/// A memory is scored only if at least this share of its words are
/// [`ENGLISH_MARKERS`], i.e. it reads as English (see [`reads_as_english`]).
const MIN_ENGLISH_MARKER_SHARE: f64 = 0.15;
/// At most this many memories are injected for one prompt.
const MAX_INJECTED_MEMORIES: usize = 5;
/// Each injected memory is cut to this many characters, so a stored
/// transcript never lands in the prompt whole.
const MAX_MEMORY_CHARS: usize = 400;

/// The SMART information-retrieval system's English stop list (Salton,
/// Cornell; 571 words), verbatim from
/// <https://github.com/igorbrigadir/stopwords/blob/master/en/smart.txt>.
/// A standard list rather than a hand-grown one: any two function words
/// missing from a short list would make a chit-chat transcript "relevant".
const SMART_STOP_WORDS: &str = include_str!("smart_stop_words.txt");

/// Words that carry no topic here beyond the SMART list: the words of the
/// `Agent <name>:` prefix every stored learning carries.
const EXTRA_STOP_WORDS: &[&str] = &["agent", "agents", "general"];

/// SMART words kept as topic words. SMART was built for news retrieval;
/// these are everyday developer vocabulary (`value`, `name`, `self`), and
/// dropping them left prompts such as `/fix the value name of the first
/// test` with nothing to match on.
const DEV_VOCABULARY: &[&str] = &[
    "example", "first", "help", "last", "name", "new", "second", "self", "value",
];

fn is_stop_word(word: &str) -> bool {
    static STOP_WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    STOP_WORDS
        .get_or_init(|| {
            SMART_STOP_WORDS
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !DEV_VOCABULARY.contains(line))
                .chain(EXTRA_STOP_WORDS.iter().copied())
                .collect()
        })
        .contains(word)
}

/// Frequent English function words that are not also common words in
/// Spanish, French, German, Italian or Portuguese (so not `a`, `de`, `la`,
/// `no`, `in`, `an`, `so`, `was`, `also`, `will`, `me`, `on`, `do`, `as`).
/// Their share of a text's words is a cheap language check, the "common
/// words" method of language identification (Grefenstette, *Comparing two
/// language identification schemes*, JADT 1995).
const ENGLISH_MARKERS: &[&str] = &[
    "about", "after", "and", "any", "are", "at", "be", "because", "been", "before", "between",
    "but", "by", "can", "could", "did", "does", "each", "for", "from", "had", "has", "have", "he",
    "her", "here", "his", "how", "if", "into", "is", "it", "its", "just", "more", "most", "must",
    "my", "not", "now", "of", "off", "only", "or", "other", "our", "out", "over", "she", "should",
    "some", "than", "that", "the", "their", "them", "then", "there", "these", "they", "this",
    "those", "to", "too", "up", "us", "very", "we", "were", "what", "when", "where", "which",
    "while", "who", "why", "with", "without", "would", "you", "your",
];

/// The agents `prompt` invokes, by name or by slash command.
fn prompt_agents(prompt: &str) -> Vec<String> {
    let mut agent_types = detect_agent_references(prompt);
    if let Some(agent) = detect_slash_command_agent(prompt)
        && !agent_types.iter().any(|existing| existing == agent)
    {
        agent_types.push(agent.to_string());
    }
    agent_types
}

pub(crate) fn inject_memory(prompt: &str, session_id: Option<&str>) -> Option<String> {
    let agent_types = prompt_agents(prompt);
    if agent_types.is_empty() {
        return None;
    }

    let session_id_owned: String;
    let session_id = match session_id.filter(|value| !value.trim().is_empty()) {
        Some(id) => id,
        None => {
            session_id_owned = generate_unique_session_id();
            tracing::warn!(
                "No session_id provided to user_prompt memory hook; using generated fallback: {}",
                session_id_owned
            );
            &session_id_owned
        }
    };
    let query_text = prompt.chars().take(500).collect::<String>();

    match retrieve_prompt_context_memories(session_id, &query_text, 2000) {
        Ok(memories) => format_agent_memory_context(&query_text, &agent_types, &memories),
        Err(error) => {
            tracing::warn!("Memory injection failed: {}", error);
            None
        }
    }
}

/// Format the memories relevant to `prompt` as one context section.
///
/// Each memory is scored against the prompt with [`memory_relevance`];
/// memories below [`RELEVANCE_THRESHOLD`] or sharing fewer than
/// [`MIN_SHARED_TERMS`] topic words are dropped. Memories with the same
/// text, ignoring the `Agent <name>:` prefix and whitespace, are printed
/// once, and each entry is bounded to [`MAX_MEMORY_CHARS`]. Returns `None` when no
/// memory is relevant, so nothing is injected.
pub fn format_agent_memory_context(
    prompt: &str,
    agent_types: &[String],
    memories: &[PromptContextMemory],
) -> Option<String> {
    // The agent names (and the slash commands that invoke them) are how the
    // prompt reached this hook, not what it is about.
    let mut ignored: HashSet<String> = HashSet::new();
    for agent in agent_types {
        ignored.extend(topic_terms(agent, &HashSet::new()));
        for command in slash_commands_for(agent) {
            ignored.insert(command.to_string());
        }
    }
    let prompt_terms = topic_terms(prompt, &ignored);

    let mut scored: Vec<(f64, String, &PromptContextMemory)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for memory in memories {
        if !reads_as_english(strip_agent_prefix(&memory.content)) {
            continue;
        }
        // Deduplicate on the whole text with whitespace collapsed, so copies
        // that differ only in layout are one memory, while memories that
        // differ anywhere (even past the printed cut) stay distinct.
        let text = single_line(strip_agent_prefix(&memory.content));
        if seen.contains(&text) {
            continue;
        }
        let memory_terms = topic_terms(&text, &ignored);
        let shared = prompt_terms.intersection(&memory_terms).count();
        let relevance = memory_relevance(&prompt_terms, &memory_terms);
        if shared >= MIN_SHARED_TERMS && relevance >= RELEVANCE_THRESHOLD {
            seen.insert(text.clone());
            scored.push((relevance, text, memory));
        }
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|left, right| right.0.total_cmp(&left.0));
    scored.truncate(MAX_INJECTED_MEMORIES);

    let heading = if agent_types.is_empty() {
        "\n## Relevant Memory\n".to_string()
    } else {
        format!(
            "\n## Relevant Memory (agents: {})\n",
            agent_types.join(", ")
        )
    };
    let mut lines = vec![heading];
    for (relevance, text, memory) in scored {
        lines.push(format!(
            "- {} (relevance: {relevance:.2})",
            bounded_memory_text(&text)
        ));
        if let Some(code_context) = memory.code_context.as_deref()
            && !code_context.trim().is_empty()
        {
            lines.push(code_context.to_string());
        }
    }
    Some(lines.join("\n"))
}

/// The memory without the `Agent <name>: ` prefix session-stop stores it
/// with. The same summary is stored once per agent, so the prefix must not
/// count toward relevance or make copies look distinct.
fn strip_agent_prefix(content: &str) -> &str {
    let content = content.trim();
    content
        .strip_prefix("Agent ")
        .and_then(|rest| rest.split_once(": "))
        .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
        .map_or(content, |(_, body)| body.trim())
}

/// Transcript role labels (`user:` / `assistant:`) are structure, not
/// words of the memory.
fn without_role_labels(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
        .filter(|token| !matches!(*token, "user:" | "assistant:"))
}

/// The words of `text`: split on anything but letters, digits and
/// apostrophes, with contractions (`None` from [`without_apostrophes`])
/// kept as `None`.
fn words(text: &str) -> impl Iterator<Item = Option<&str>> {
    without_role_labels(text)
        .flat_map(|token| token.split(|c: char| !c.is_alphanumeric() && !is_apostrophe(c)))
        .filter(|raw| !raw.is_empty())
        .map(without_apostrophes)
}

/// Whether `text` reads as English: at least [`MIN_ENGLISH_MARKER_SHARE`]
/// of its words are [`ENGLISH_MARKERS`].
///
/// The relevance filter only knows English. Another language's function
/// words (`schon`, `jetzt`, `porque`, `可能`) would be topic words to it, so
/// two unrelated sentences in that language could look relevant; such a
/// memory is not scored at all. English memories that are bare keyword
/// lists (`cargo fmt`) fail the check too. Both fail closed.
fn reads_as_english(text: &str) -> bool {
    let (mut total, mut english) = (0usize, 0usize);
    for word in words(text) {
        total += 1;
        if word.is_some_and(|word| ENGLISH_MARKERS.contains(&word.to_lowercase().as_str())) {
            english += 1;
        }
    }
    total > 0 && english as f64 / total as f64 >= MIN_ENGLISH_MARKER_SHARE
}

/// Distinct lower-cased topic words of `text`, without stop words, short
/// words or `ignored` words.
///
/// Only English is understood: stop words are the SMART list, and a word
/// with an apostrophe other than a possessive `'s` is a contraction
/// (`doesn't`, `they'll`, `you'd`) and never a topic. There is no stemming:
/// `tests` does not match `test`.
///
/// Known limit: only ASCII words are topic words. Accented Latin, Cyrillic,
/// Greek, and Chinese, Japanese and Korean text (which has no spaces
/// between words, or attaches grammar to them) contribute nothing, and
/// non-English memories are not scored (see [`reads_as_english`]). Memories
/// in other languages are therefore never injected: this fails closed —
/// nothing irrelevant is injected, a relevant memory may be missed.
fn topic_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
    words(text)
        .flatten()
        .filter(|word| {
            word.len() >= MIN_TERM_CHARS && word.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(str::to_lowercase)
        .filter(|word| !is_stop_word(word) && !ignored.contains(word))
        .collect()
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// `raw` with surrounding quotes and a possessive `'s` dropped
/// (`builder's` → `builder`), or `None` when an apostrophe remains: the
/// word is a contraction (`doesn't`, `we'll`, `you'd`), which carries no
/// topic, and whose apostrophe-less spelling can collide with a real word
/// (`we'll` → `well`).
fn without_apostrophes(raw: &str) -> Option<&str> {
    let word = raw.trim_matches(is_apostrophe);
    let word = word
        .char_indices()
        .rev()
        .nth(1)
        .filter(|(index, c)| {
            is_apostrophe(*c) && word[index + c.len_utf8()..].eq_ignore_ascii_case("s")
        })
        .map_or(word, |(index, _)| &word[..index]);
    (!word.contains(is_apostrophe)).then_some(word)
}

/// Cosine similarity of the prompt's and the memory's topic-word sets, in
/// `[0, 1]`. Dividing by both set sizes keeps a long transcript, which shares
/// some words with almost any prompt, from scoring as relevant.
fn memory_relevance(prompt_terms: &HashSet<String>, memory_terms: &HashSet<String>) -> f64 {
    if prompt_terms.is_empty() || memory_terms.is_empty() {
        return 0.0;
    }
    let shared = prompt_terms.intersection(memory_terms).count() as f64;
    shared / ((prompt_terms.len() * memory_terms.len()) as f64).sqrt()
}

/// `content` with every run of whitespace collapsed to one space.
fn single_line(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A single-line memory cut to [`MAX_MEMORY_CHARS`] characters.
fn bounded_memory_text(line: &str) -> String {
    if line.chars().count() <= MAX_MEMORY_CHARS {
        return line.to_string();
    }
    let mut cut = line.chars().take(MAX_MEMORY_CHARS).collect::<String>();
    cut.push('…');
    cut
}

/// Generate a unique session ID from PID and current timestamp.
fn generate_unique_session_id() -> String {
    format!(
        "hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

/// Check if AMPLIHACK.md should be injected (differs from CLAUDE.md).
pub(crate) fn check_framework_injection(dirs: &ProjectDirs) -> Option<String> {
    let amplihack_path = find_amplihack_md(dirs)?;
    let claude_path = dirs.claude_md();

    let amplihack_content = fs::read_to_string(&amplihack_path).ok()?;
    let claude_content = fs::read_to_string(&claude_path).ok().unwrap_or_default();

    // Normalize whitespace for comparison.
    let norm_amplihack: String = amplihack_content.split_whitespace().collect();
    let norm_claude: String = claude_content.split_whitespace().collect();

    if norm_amplihack == norm_claude {
        return None; // Already identical.
    }

    Some(amplihack_content)
}

fn find_amplihack_md(dirs: &ProjectDirs) -> Option<PathBuf> {
    // Check CLAUDE_PLUGIN_ROOT env var first.
    if let Ok(root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        let path = PathBuf::from(root).join("AMPLIHACK.md");
        if path.exists() {
            return Some(path);
        }
    }

    // Check .claude/AMPLIHACK.md.
    let path = dirs.amplihack_md();
    if path.exists() {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(content: &str) -> PromptContextMemory {
        PromptContextMemory {
            content: content.to_string(),
            code_context: None,
        }
    }

    fn agents(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn relevant_memory_is_injected_with_its_computed_score() {
        let result = format_agent_memory_context(
            "/analyze why cargo test fails in CI",
            &agents(&["analyzer"]),
            &[memory("Always run cargo test before pushing to CI")],
        )
        .expect("relevant memory is injected");
        assert!(result.contains("## Relevant Memory (agents: analyzer)"));
        assert!(result.contains("Always run cargo test before pushing to CI"));
        // `analyze` invokes the agent, `ci` is too short and `always` /
        // `before` are stop words, leaving {cargo, test, fails} vs {run,
        // cargo, test, pushing}: 2 shared / sqrt(3 * 4).
        let expected = 2.0 / 12f64.sqrt();
        assert!(result.contains(&format!("(relevance: {expected:.2})")));
        assert!(!result.contains("relevance: 0.00"));
    }

    #[test]
    fn irrelevant_memory_injects_nothing() {
        let result = format_agent_memory_context(
            "/analyze the auth middleware",
            &agents(&["analyzer"]),
            &[memory("user: reply with just: pong\nassistant: pong")],
        );
        assert_eq!(result, None);
    }

    #[test]
    fn no_memories_injects_nothing() {
        assert_eq!(
            format_agent_memory_context("/analyze auth", &agents(&["analyzer"]), &[]),
            None
        );
    }

    #[test]
    fn each_memory_is_printed_once_whatever_the_agent_count() {
        let result = format_agent_memory_context(
            "review the builder output for cargo fmt failures",
            &agents(&["builder", "reviewer", "tester"]),
            &[
                memory("cargo fmt failures come from the builder output"),
                memory("cargo fmt failures come from the builder output"),
            ],
        )
        .expect("relevant memory is injected");
        assert_eq!(result.matches("## Relevant Memory").count(), 1);
        assert_eq!(
            result
                .matches("cargo fmt failures come from the builder output")
                .count(),
            1
        );
        assert!(result.contains("builder, reviewer, tester"));
    }

    /// Session-stop stores one `Agent <name>: <summary>` copy per agent.
    #[test]
    fn per_agent_prefixed_copies_are_printed_once_without_prefix() {
        let body = "cargo fmt failures come from the builder output";
        let memories = ["builder", "reviewer", "tester", "general"]
            .map(|agent| memory(&format!("Agent {agent}: {body}")));
        let result = format_agent_memory_context(
            "why does cargo fmt report failures",
            &agents(&["builder"]),
            &memories,
        )
        .expect("relevant memory is injected");
        assert_eq!(result.matches(body).count(), 1);
        assert!(!result.contains("Agent "));
    }

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

    /// Memories that share their first [`MAX_MEMORY_CHARS`] characters but
    /// differ later are distinct, and each keeps its own code context.
    #[test]
    fn memories_differing_past_the_cut_are_both_kept() {
        let shared = format!("cargo fmt failures {}", "the detail ".repeat(40));
        let result = format_agent_memory_context(
            "cargo fmt failures detail",
            &agents(&["builder"]),
            &[
                PromptContextMemory {
                    content: format!("{shared} first ending"),
                    code_context: Some("ctx-one".to_string()),
                },
                PromptContextMemory {
                    content: format!("{shared} second ending"),
                    code_context: Some("ctx-two".to_string()),
                },
            ],
        )
        .expect("relevant memories are injected");
        assert_eq!(
            result.lines().filter(|line| line.starts_with("- ")).count(),
            2
        );
        assert!(result.contains("ctx-one"));
        assert!(result.contains("ctx-two"));
    }

    /// Copies that differ only in whitespace print the same line, so they
    /// are one memory.
    #[test]
    fn whitespace_variant_copies_are_printed_once() {
        let result = format_agent_memory_context(
            "why does cargo fmt report failures in the builder",
            &agents(&["analyzer"]),
            &[
                memory("Agent a: cargo fmt failures\nfrom the builder"),
                memory("Agent b: cargo fmt failures from the builder"),
                memory("cargo  fmt failures   from the builder "),
            ],
        )
        .expect("relevant memory is injected");
        assert_eq!(
            result
                .matches("- cargo fmt failures from the builder")
                .count(),
            1
        );
    }

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
    }

    #[test]
    fn empty_agent_list_has_a_plain_heading() {
        let result =
            format_agent_memory_context("cargo fmt", &[], &[memory("the cargo fmt")]).unwrap();
        assert!(result.contains("## Relevant Memory\n"));
        assert!(!result.contains("agents:"));
    }

    #[test]
    fn agent_prefix_is_stripped_only_when_it_is_one() {
        assert_eq!(strip_agent_prefix("Agent builder: body"), "body");
        assert_eq!(
            strip_agent_prefix("Agent smith said: hello"),
            "Agent smith said: hello"
        );
        assert_eq!(strip_agent_prefix("plain memory"), "plain memory");
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

    #[test]
    fn injected_memory_is_bounded() {
        let long = format!("cargo test ci {}", "the detail ".repeat(250));
        let result = format_agent_memory_context(
            "cargo test ci detail",
            &agents(&["tester"]),
            &[memory(&long)],
        )
        .expect("relevant memory is injected");
        let entry = result
            .lines()
            .find(|line| line.starts_with("- "))
            .expect("memory entry");
        assert!(entry.contains('…'));
        assert!(entry.chars().count() < MAX_MEMORY_CHARS + 40);
    }

    #[test]
    fn at_most_max_memories_are_injected_most_relevant_first() {
        let mut memories = (0..MAX_INJECTED_MEMORIES + 3)
            .map(|index| memory(&format!("the cargo fmt note{index} extra{index}")))
            .collect::<Vec<_>>();
        memories.push(memory("the cargo fmt"));
        let result =
            format_agent_memory_context("cargo fmt", &agents(&["builder"]), &memories).unwrap();
        let entries = result
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), MAX_INJECTED_MEMORIES);
        assert!(entries[0].starts_with("- the cargo fmt (relevance: 1.00)"));
    }

    #[test]
    fn format_with_code_context() {
        let result = format_agent_memory_context(
            "important fact about main",
            &agents(&["builder"]),
            &[PromptContextMemory {
                content: "Important fact about main".to_string(),
                code_context: Some("fn main() {}".to_string()),
            }],
        )
        .unwrap();
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn format_empty_code_context_skipped() {
        let result = format_agent_memory_context(
            "cargo fact",
            &agents(&["builder"]),
            &[PromptContextMemory {
                content: "The cargo fact".to_string(),
                code_context: Some("  ".to_string()),
            }],
        )
        .unwrap();
        assert!(!result.contains("  \n"));
        assert!(!result.ends_with("  "));
    }
}
