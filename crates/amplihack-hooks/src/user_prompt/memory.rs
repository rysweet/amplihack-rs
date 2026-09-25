//! Agent memory injection and framework file detection.

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, slash_commands_for,
};
use amplihack_memory::cli_memory::{PromptContextMemory, retrieve_prompt_context_memories};
use amplihack_types::ProjectDirs;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

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
/// Han and katakana are split into character pairs instead and Hangul words
/// need two syllables (see [`topic_terms`]).
const MIN_TERM_CHARS: usize = 3;
/// At most this many memories are injected for one prompt.
const MAX_INJECTED_MEMORIES: usize = 5;
/// Each injected memory is cut to this many characters, so a stored
/// transcript never lands in the prompt whole.
const MAX_MEMORY_CHARS: usize = 400;

/// Words that carry no topic: English function words, the role labels
/// stored transcripts use (`user:` / `assistant:`), and the words of the
/// `Agent <name>:` prefix every stored learning carries.
const STOP_WORDS: &[&str] = &[
    "a",
    "about",
    "agent",
    "agents",
    "after",
    "all",
    "also",
    "am",
    "an",
    "and",
    "any",
    "are",
    "as",
    "assistant",
    "at",
    "be",
    "been",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "for",
    "from",
    "general",
    "had",
    "has",
    "have",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "just",
    "me",
    "my",
    "no",
    "not",
    "now",
    "of",
    "on",
    "or",
    "our",
    "please",
    "should",
    "so",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "this",
    "those",
    "to",
    "up",
    "us",
    "use",
    "user",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
    // English contractions, as `topic_terms` spells them once the
    // apostrophe is gone (`don't` → `dont`; `'s` is dropped first).
    "aren",
    "arent",
    "cannot",
    "cant",
    "couldnt",
    "didnt",
    "doesnt",
    "dont",
    "hadnt",
    "hasnt",
    "havent",
    "isnt",
    "ive",
    "let",
    "shouldnt",
    "theyre",
    "theyve",
    "wasnt",
    "werent",
    "weve",
    "wont",
    "wouldnt",
    "youll",
    "youre",
    "youve",
    // Korean words that carry no topic: conjunctions, pronouns and the
    // polite sentence endings that stand as their own word.
    "그리고",
    "그러나",
    "하지만",
    "그래서",
    "그런데",
    "그것",
    "이것",
    "저것",
    "우리",
    "저는",
    "제가",
    "나는",
    "무엇",
    "어떻게",
    "합니다",
    "했습니다",
    "입니다",
    "있습니다",
    "없습니다",
    "됩니다",
    "주세요",
];

/// Chinese and Japanese characters that are grammar rather than topic
/// (particles, pronouns, copulas, measure words). A Han pair containing one
/// (`失败了` → `败了`) would match any two sentences, so it is not a term.
const HAN_FUNCTION_CHARS: &str = "的了是在我你他她它们这那吗呢吧啊和与也就都要会有不没一个把被对从到给还又很让说之其以而及或如但并为什么事中方";

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

/// Distinct lower-cased topic words of `text`, without stop words, short
/// words or `ignored` words.
///
/// Chinese and Japanese are written without spaces, so whitespace splitting
/// would turn a whole sentence into one term that never matches:
/// - Han and katakana runs contribute overlapping character pairs (`构建失败`
///   → `构建`, `建失`, `失败`), the usual segmentation-free approximation.
///   Han pairs containing a grammatical character ([`HAN_FUNCTION_CHARS`])
///   are dropped.
/// - Hiragana is dropped. In Japanese it carries particles and verb endings
///   (`が`, `しました`), whose pairs would match any two sentences.
/// - Hangul is space-separated; each word of two or more syllables is a term,
///   except the grammatical words in [`STOP_WORDS`].
///
/// Known limit: Korean words keep their attached particles (`빌드가` ≠ `빌드`),
/// and Thai, Lao, Khmer and Myanmar, which also lack spaces, are not split, so
/// memories in those scripts rarely match. Both fail closed: nothing
/// irrelevant is injected, a relevant memory may be missed.
fn topic_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
    let mut terms = HashSet::new();
    let mut keep = |term: String| {
        if !STOP_WORDS.contains(&term.as_str()) && !ignored.contains(&term) {
            terms.insert(term);
        }
    };
    for raw in text.split(|c: char| !c.is_alphanumeric() && !is_apostrophe(c)) {
        let chars = without_apostrophes(raw).chars().collect::<Vec<_>>();
        for run in chars.chunk_by(|left, right| Script::of(*left) == Script::of(*right)) {
            match Script::of(run[0]) {
                Script::Han => {
                    for pair in run.windows(2) {
                        if !pair.iter().any(|c| HAN_FUNCTION_CHARS.contains(*c)) {
                            keep(pair.iter().collect());
                        }
                    }
                }
                Script::Katakana => {
                    for pair in run.windows(2) {
                        keep(pair.iter().collect());
                    }
                }
                Script::Hiragana => {}
                Script::Hangul if run.len() >= 2 => keep(run.iter().collect()),
                Script::Hangul => {}
                Script::Other if run.len() >= MIN_TERM_CHARS => {
                    keep(run.iter().collect::<String>().to_lowercase());
                }
                Script::Other => {}
            }
        }
    }
    terms
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// `raw` with a possessive `'s` dropped and other apostrophes removed, so
/// `builder's` is `builder` and `doesn't` is one word, `doesnt`, not a stray
/// `doesn` that would count as a topic.
fn without_apostrophes(raw: &str) -> String {
    let word = raw.trim_matches(is_apostrophe);
    let word = word
        .char_indices()
        .rev()
        .nth(1)
        .filter(|(index, c)| {
            is_apostrophe(*c) && word[index + c.len_utf8()..].eq_ignore_ascii_case("s")
        })
        .map_or(word, |(index, _)| &word[..index]);
    word.chars().filter(|c| !is_apostrophe(*c)).collect()
}

/// The scripts [`topic_terms`] tokenises differently from space-separated
/// text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Other,
}

impl Script {
    fn of(c: char) -> Self {
        match c as u32 {
            0x3040..=0x309F => Self::Hiragana,
            0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Self::Katakana,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Self::Hangul,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF => Self::Han,
            _ => Self::Other,
        }
    }
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
        // `analyze` invokes the agent and `ci` is too short, leaving
        // {cargo, test, fails} vs {always, run, cargo, test, before,
        // pushing}: 2 shared / sqrt(3 * 6).
        let expected = 2.0 / 18f64.sqrt();
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
        let shared = format!("cargo fmt failures {}", "detail ".repeat(80));
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
                memory("Agent a: cargo fmt failures\nbuilder"),
                memory("Agent b: cargo fmt failures builder"),
                memory("cargo  fmt failures   builder "),
            ],
        )
        .expect("relevant memory is injected");
        assert_eq!(result.matches("- cargo fmt failures builder").count(), 1);
    }

    #[test]
    fn cjk_memory_is_injected_for_a_cjk_prompt() {
        let result = format_agent_memory_context(
            "/analyze 构建失败",
            &agents(&["analyzer"]),
            &[
                memory("Agent analyzer: 构建失败时先运行 cargo fmt"),
                memory("Agent analyzer: 部署成功"),
            ],
        )
        .expect("CJK memory is injected");
        assert!(result.contains("构建失败时先运行 cargo fmt"));
        assert!(!result.contains("部署成功"));
    }

    /// Japanese and Korean verb endings and particles are not topic words:
    /// sentences that share only those match nothing.
    #[test]
    fn japanese_and_korean_endings_do_not_make_memories_relevant() {
        for (prompt, unrelated) in [
            (
                "/analyze ビルドが失敗しました",
                "Agent analyzer: デプロイが成功しました",
            ),
            (
                "/analyze 빌드가 실패했습니다",
                "Agent analyzer: 배포가 성공했습니다",
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
        let prompt = "/analyze 我们的构建失败了吗";
        assert_eq!(
            format_agent_memory_context(
                prompt,
                &prompt_agents(prompt),
                &[memory("Agent analyzer: 我们的部署成功了吗")]
            ),
            None
        );
    }

    /// Contractions are one stop word, not a stray `doesn` / `isn` topic.
    #[test]
    fn contractions_do_not_make_memories_relevant() {
        let prompt = "/analyze why doesn't it work, isn't it wired?";
        assert_eq!(
            format_agent_memory_context(
                prompt,
                &prompt_agents(prompt),
                &[memory(
                    "Agent analyzer: the cache doesn't expire and isn't cleared"
                )]
            ),
            None
        );
        assert_eq!(without_apostrophes("builder's"), "builder");
        assert_eq!(without_apostrophes("doesn\u{2019}t"), "doesnt");
        assert_eq!(without_apostrophes("'quoted'"), "quoted");
        assert_eq!(without_apostrophes("s"), "s");
    }

    #[test]
    fn japanese_memory_is_injected_for_a_japanese_prompt() {
        let prompt = "/analyze ビルドが失敗しました";
        let result = format_agent_memory_context(
            prompt,
            &prompt_agents(prompt),
            &[memory(
                "Agent analyzer: ビルド失敗の原因は cargo fmt でした",
            )],
        )
        .expect("Japanese memory is injected");
        assert!(result.contains("ビルド失敗の原因は cargo fmt でした"));
    }

    #[test]
    fn space_separated_short_korean_words_count() {
        let result = format_agent_memory_context(
            "/analyze 빌드 실패",
            &agents(&["analyzer"]),
            &[memory("Agent analyzer: 빌드 실패 원인은 포맷")],
        );
        assert!(result.is_some_and(|text| text.contains("빌드 실패 원인은 포맷")));
    }

    #[test]
    fn empty_agent_list_has_a_plain_heading() {
        let result = format_agent_memory_context("cargo fmt", &[], &[memory("cargo fmt")]).unwrap();
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
        let long = format!("cargo test ci {}", "detail ".repeat(500));
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
            .map(|index| memory(&format!("cargo fmt note{index} extra{index} more{index}")))
            .collect::<Vec<_>>();
        memories.push(memory("cargo fmt"));
        let result =
            format_agent_memory_context("cargo fmt", &agents(&["builder"]), &memories).unwrap();
        let entries = result
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), MAX_INJECTED_MEMORIES);
        assert!(entries[0].starts_with("- cargo fmt (relevance: 1.00)"));
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
                content: "Cargo fact".to_string(),
                code_context: Some("  ".to_string()),
            }],
        )
        .unwrap();
        assert!(!result.contains("  \n"));
        assert!(!result.ends_with("  "));
    }
}
