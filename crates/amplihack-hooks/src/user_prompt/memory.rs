//! Agent memory injection and framework file detection.

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, slash_commands_for,
    without_definition_references,
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
/// A memory turn contributes topic words only if at least this share of
/// its prose words are [`ENGLISH_MARKERS`] or English contractions, i.e. it
/// reads as English (see [`reads_as_english`]).
const MIN_ENGLISH_MARKER_SHARE: f64 = 0.1;
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

/// Frequent English function words that are not also frequent words in
/// other Latin-script languages. Removed for that reason:
/// - `a`, `de`, `la`, `no`, `he`, `has`, `on`, `me`, `do`, `as` (Spanish,
///   Catalan, French, Italian, Portuguese), `any`, `us` (Catalan),
/// - `in`, `an`, `so`, `was`, `also`, `will`, `am` (German),
/// - `is`, `of`, `we`, `had`, `over` (Dutch),
/// - `at`, `for`, `her`, `have`, `i` (Danish, Norwegian, Tagalog),
/// - `just` (Swedish), `most`, `be` (Hungarian),
/// - `to`, `my`, `by` (Polish, Czech, Slovak).
///
/// No word left is in the stopwords-iso list
/// (<https://github.com/stopwords-iso/stopwords-iso>) of Afrikaans,
/// Basque, Catalan, Croatian, Czech, Danish, Dutch, Esperanto, Estonian,
/// Finnish, French, Galician, German, Hausa, Hungarian, Indonesian, Irish,
/// Italian, Latin, Malay, Norwegian, Polish, Portuguese, Slovak, Slovenian,
/// Somali, Sotho, Spanish, Swahili, Swedish, Tagalog, Turkish, Vietnamese,
/// Yoruba or Zulu. Known remaining collisions, kept because removing them
/// stops realistic English notes from reading as English: `are`, `or`
/// (Romanian), `it` (Latvian, Lithuanian), `it`, `out`, `you` (Breton).
///
/// Their share of a text's prose words is a cheap language check, the
/// "common words" method of language identification (Grefenstette,
/// *Comparing two language identification schemes*, JADT 1995).
const ENGLISH_MARKERS: &[&str] = &[
    "about", "after", "and", "are", "because", "been", "before", "between", "but", "can", "could",
    "did", "does", "each", "from", "here", "his", "how", "if", "into", "it", "its", "more", "must",
    "not", "now", "off", "only", "or", "other", "our", "out", "she", "should", "some", "than",
    "that", "the", "their", "them", "then", "there", "these", "they", "this", "those", "too", "up",
    "very", "were", "what", "when", "where", "which", "while", "who", "why", "with", "without",
    "would", "you", "your",
];

/// Endings only English contractions have (`doesn't`, `we'll`, `they're`,
/// `I've`, `you'd`, `I'm`); French and Italian elisions (`c'est`, `l'acqua`)
/// put the apostrophe elsewhere.
const ENGLISH_CONTRACTION_ENDINGS: &[&str] = &["n't", "'ll", "'re", "'ve", "'d", "'m"];

/// The agent names, and the slash commands that invoke them, are how the
/// prompt reached this hook, not what it is about.
fn ignored_terms(agent_types: &[String]) -> HashSet<String> {
    let mut ignored = HashSet::new();
    for agent in agent_types {
        ignored.extend(topic_terms(agent, &HashSet::new()));
        for command in slash_commands_for(agent) {
            ignored.insert(command.to_string());
        }
    }
    ignored
}

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
    // Checked before retrieval too: a prompt that fails it gets nothing, so
    // loading the session's memories for it would be wasted.
    if !prompt_reads_as_english(
        &without_definition_references(&query_text),
        &ignored_terms(&agent_types),
    ) {
        return None;
    }

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
/// once, and each entry is bounded to [`MAX_MEMORY_CHARS`].
///
/// Returns `None`, so nothing is injected, when no memory is relevant, or
/// when the prompt itself fails [`prompt_reads_as_english`]: the filter
/// only understands English, on both sides of the comparison.
pub fn format_agent_memory_context(
    prompt: &str,
    agent_types: &[String],
    memories: &[PromptContextMemory],
) -> Option<String> {
    let ignored = ignored_terms(agent_types);
    // Agent definition references are left out of the comparison, on both
    // sides: a memory stored from a prompt that used the same reference
    // would otherwise share its directory names (`claude`, `amplihack`).
    let prompt = without_definition_references(prompt);
    // The prompt is held to the same language check as memory turns: a
    // German prompt's `die` / `bin` / `mit` would otherwise match the same
    // words in an English memory.
    if !prompt_reads_as_english(&prompt, &ignored) {
        return None;
    }
    let prompt_terms = scored_terms(&prompt, &ignored);

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
        // Only the turns that read as English are understood, so only they
        // contribute topic words: in a transcript where the user wrote
        // another language and the assistant answered in English, the
        // user's function words must not count.
        //
        // A fence that seems to span a role label is ambiguous once a
        // transcript is flattened: a pasted log inside one message (the
        // label is content) or a stray fence line in one message pairing
        // with a fence in the next (the label is a real turn boundary). The
        // memory must be relevant under both readings, so neither can merge
        // a non-English turn into an English one; without such a fence the
        // two readings are the same.
        let body = without_definition_references(strip_agent_prefix(&memory.content));
        let readings = [true, false].map(|respect_fences| {
            let memory_terms: HashSet<String> = turns_with(&body, respect_fences)
                .iter()
                .filter(|turn| reads_as_english(turn))
                .flat_map(|turn| scored_terms(turn, &ignored))
                .collect();
            (
                prompt_terms.intersection(&memory_terms).count(),
                memory_relevance(&prompt_terms, &memory_terms),
            )
        });
        let shared = readings[0].0.min(readings[1].0);
        let relevance = readings[0].1.min(readings[1].1);
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

/// The words of `text`: split on anything but letters, digits and
/// apostrophes, with contractions (`None` from [`without_apostrophes`])
/// kept as `None`.
fn words(text: &str) -> impl Iterator<Item = Option<&str>> {
    text.split_whitespace()
        .flat_map(|token| token.split(|c: char| !c.is_alphanumeric() && !is_apostrophe(c)))
        .filter(|raw| !raw.is_empty())
        .map(without_apostrophes)
}

/// `text` as prose and code parts, following CommonMark (spec 0.31.2):
/// fenced blocks (§4.5) are found first, in one pass over lines, and code
/// spans (§6.1) in the prose between them. Code parts are tagged `true`.
/// A delimiter with no closing partner (a stray backtick, or a block cut
/// off by session-stop's 500-character head) opens nothing: the text after
/// it stays prose.
fn segments(text: &str) -> Vec<(bool, &str)> {
    let mut segments = Vec::new();
    for (in_fence, part) in fenced_blocks(text) {
        if in_fence {
            segments.push((true, part));
        } else {
            segments.extend(backtick_spans(part));
        }
    }
    segments
}

/// `text` split at fenced code blocks (CommonMark §4.5). A line that starts
/// with three or more backticks or tildes, after any container markers (see
/// [`fence_run`]), opens a fence (a backtick fence's info string may not
/// contain a backtick). The next line in the same container holding at
/// least as many of the same character and nothing else but whitespace
/// closes it. Everything between is code, fence-like lines of the other
/// character included.
fn fenced_blocks(text: &str) -> Vec<(bool, &str)> {
    let mut parts = Vec::new();
    let mut prose_start = 0;
    for fence in fences(text) {
        parts.push((false, &text[prose_start..fence.start]));
        parts.push((true, &text[fence.content.clone()]));
        prose_start = fence.end;
    }
    parts.push((false, &text[prose_start..]));
    parts
}

/// A closed fenced block in a text: its byte range from the opening line's
/// start to the closing line's end, and the range of its content.
struct Fence {
    start: usize,
    content: std::ops::Range<usize>,
    end: usize,
}

/// The closed fenced blocks of `text`, in order (see [`fenced_blocks`]).
fn fences(text: &str) -> Vec<Fence> {
    fences_in(text, text)
}

/// The closed fenced blocks of `text`, with openers looked for in
/// `openers`, a copy of `text` of the same length (with role labels
/// blanked, say), and closers in `text` itself: a line inside a fence is
/// content, whatever it starts with.
fn fences_in(text: &str, openers: &str) -> Vec<Fence> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for (line, opener_line) in text
        .split_inclusive('\n')
        .zip(openers.split_inclusive('\n'))
    {
        lines.push((offset, line, opener_line));
        offset += line.len();
    }
    let mut fences = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (open_at, open_line, opener_line) = lines[index];
        let opener = fence_run(opener_line)
            .filter(|opener| !(opener.marker == '`' && opener.rest.contains('`')));
        // A closer sits in the same container as its opener: the same
        // number of `>` markers, and never behind a list-item marker (a list
        // item's closer is indented instead). Inside a fence, a `- ```` diff
        // line or a ` * ```` doc-comment line is content.
        let close = opener.and_then(|opener| {
            lines[index + 1..].iter().position(|(_, line, _)| {
                fence_run(line).is_some_and(|closer| {
                    closer.marker == opener.marker
                        && closer.width >= opener.width
                        && closer.rest.trim().is_empty()
                        && closer.quote_depth == opener.quote_depth
                        && !closer.list_item
                })
            })
        });
        let Some(close) = close else {
            index += 1;
            continue;
        };
        let (close_at, close_line, _) = lines[index + 1 + close];
        fences.push(Fence {
            start: open_at,
            content: open_at + open_line.len()..close_at,
            end: close_at + close_line.len(),
        });
        index += close + 2;
    }
    fences
}

/// A line that starts with a fence run, after indentation and any
/// block-quote (`>`) or list-item (`-`, `*`, `+`, `1.`, `1)`) markers, since
/// a fence inside those containers is still a fence.
struct FenceLine<'a> {
    /// `` ` `` or `~`.
    marker: char,
    /// The run's length, at least 3.
    width: usize,
    /// The rest of the line after the run.
    rest: &'a str,
    /// How many `>` block-quote markers came before the run.
    quote_depth: usize,
    /// Whether a list-item marker came before the run.
    list_item: bool,
}

/// The fence run `line` starts with, if any (see [`FenceLine`]).
fn fence_run(line: &str) -> Option<FenceLine<'_>> {
    let mut line = line.trim_start();
    let mut quote_depth = 0;
    while let Some(rest) = line.strip_prefix('>') {
        line = rest.trim_start();
        quote_depth += 1;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let list_marker = if line.starts_with(['-', '*', '+']) {
        Some(1)
    } else if (1..=9).contains(&digits) && line[digits..].starts_with(['.', ')']) {
        Some(digits + 1)
    } else {
        None
    };
    let mut list_item = false;
    if let Some(marker_len) = list_marker
        && line[marker_len..].starts_with(char::is_whitespace)
    {
        line = line[marker_len..].trim_start();
        list_item = true;
    }
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let width = line.chars().take_while(|c| *c == marker).count();
    (width >= 3).then(|| FenceLine {
        marker,
        width,
        rest: &line[width..],
        quote_depth,
        list_item,
    })
}

/// `text` split at backtick code spans: a run of backticks opens a span
/// that the next run of exactly as many closes.
fn backtick_spans(text: &str) -> Vec<(bool, &str)> {
    let bytes = text.as_bytes();
    let run_end = |from: usize| {
        let mut end = from;
        while end < bytes.len() && bytes[end] == b'`' {
            end += 1;
        }
        end
    };
    let mut parts = Vec::new();
    let (mut prose_start, mut index) = (0, 0);
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let open_end = run_end(index);
        let width = open_end - index;
        let mut scan = open_end;
        let mut close = None;
        while scan < bytes.len() {
            if bytes[scan] == b'`' {
                let end = run_end(scan);
                if end - scan == width {
                    close = Some((scan, end));
                    break;
                }
                scan = end;
            } else {
                scan += 1;
            }
        }
        match close {
            Some((close_start, close_end)) => {
                parts.push((false, &text[prose_start..index]));
                parts.push((true, &text[open_end..close_start]));
                prose_start = close_end;
                index = close_end;
            }
            None => index = open_end,
        }
    }
    parts.push((false, &text[prose_start..]));
    parts
}

/// The roles a Claude- or OpenAI-style transcript entry can carry, which
/// session-stop writes as `<role>: ` labels.
const TRANSCRIPT_ROLES: &[&str] = &[
    "assistant",
    "developer",
    "function",
    "human",
    "system",
    "tool",
    "user",
];

/// `text` with each paragraph-opening role label (`user: `) replaced by as
/// many spaces, so byte offsets are unchanged.
fn without_labels(text: &str) -> String {
    let mut masked = String::with_capacity(text.len());
    let mut paragraph_start = true;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        let label_len = line
            .trim_start()
            .split_once(": ")
            .filter(|(role, _)| paragraph_start && TRANSCRIPT_ROLES.contains(role))
            .map(|(role, _)| role.len() + 2);
        match label_len {
            Some(len) => {
                masked.push_str(&line[..indent]);
                masked.push_str(&" ".repeat(len));
                masked.push_str(&line[indent + len..]);
            }
            None => masked.push_str(line),
        }
        paragraph_start = line.trim().is_empty();
    }
    masked
}

/// `text` split into transcript turns, respecting fences (see
/// [`turns_with`]).
#[cfg(test)]
fn turns(text: &str) -> Vec<String> {
    turns_with(text, true)
}

/// `text` split into transcript turns, without their role labels.
///
/// Session-stop flattens a transcript as `<role>: <text>` paragraphs joined
/// by blank lines, with whatever role the transcript carries. A paragraph
/// that starts with one of the [`TRANSCRIPT_ROLES`] labels starts a turn;
/// other paragraphs (a code block with blank lines in it, a pasted log line
/// such as `user: …` inside a closed fenced block, when `respect_fences`,
/// or a note that opens
/// with `sqlite: …`) continue the current one and keep their words.
/// Text without labels is one turn. With `respect_fences` false, every
/// paragraph-opening label starts a turn, fence or not.
fn turns_with(text: &str, respect_fences: bool) -> Vec<String> {
    // A label inside a closed fenced block is part of the block, not a turn.
    // Fences are found with the labels blanked out (same byte offsets), so
    // a message that starts with a fence (`user: ```) opens it.
    let fences = if respect_fences {
        fences_in(text, &without_labels(text))
    } else {
        Vec::new()
    };
    let in_fence = |at: usize| fences.iter().any(|fence| fence.content.contains(&at));
    let mut turns = vec![String::new()];
    let mut paragraph_start = true;
    let mut offset = 0;
    for raw_line in text.split_inclusive('\n') {
        let at = offset;
        offset += raw_line.len();
        let line = raw_line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            paragraph_start = true;
            continue;
        }
        let label = line.trim_start().split_once(": ").filter(|(role, _)| {
            paragraph_start && !in_fence(at) && TRANSCRIPT_ROLES.contains(role)
        });
        match label {
            Some((_, rest)) => turns.push(rest.to_string()),
            None => {
                if let Some(turn) = turns.last_mut() {
                    turn.push('\n');
                    turn.push_str(line);
                }
            }
        }
        paragraph_start = false;
    }
    turns
        .into_iter()
        .map(|turn| turn.trim().to_string())
        .filter(|turn| !turn.is_empty())
        .collect()
}

/// The natural-language words of `text`, lower-cased: code is not
/// evidence of any language, so fenced blocks, closed backtick spans and
/// tokens that look like code (`src/main.rs:12`, `--test-threads=1`,
/// `needless_borrow`, `re-ran`, `DEFAULT_STEP_TIMEOUT`) are left out. Words
/// in capitals (`CI`, `THE`, `ICH`) are words like any other.
/// Surrounding punctuation and quotes in any script (`¿`, `“`, `»`, `.`)
/// are trimmed first, so they don't make a word look like code.
fn prose_words(text: &str) -> Vec<String> {
    segments(text)
        .into_iter()
        .filter(|(is_code, _)| !is_code)
        .flat_map(|(_, part)| part.split_whitespace().filter_map(prose_word))
        .collect()
}

/// `token` as a lower-cased prose word, or `None` when it looks like code.
/// Capitals alone don't make code: `ICH BIN` is still words that
/// [`topic_terms`] scores, so the language check must see them too.
fn prose_word(token: &str) -> Option<String> {
    let token = token
        .trim_start_matches(|c: char| is_prose_punctuation(c) && c != '-')
        .trim_end_matches(is_prose_punctuation);
    let looks_like_code = token
        .chars()
        .any(|c| !c.is_alphabetic() && !is_apostrophe(c));
    (!token.is_empty() && !looks_like_code).then(|| token.to_lowercase().replace('\u{2019}', "'"))
}

/// The topic words of `text` that are scored: those of its prose, and of
/// each code part unless that part is itself a sentence in another
/// language.
///
/// Code is left out of the language check, so a German error pasted in a
/// fence after `fix this:` would otherwise be scored as English. A code part
/// with at least [`MIN_WORDS_TO_JUDGE`] prose-like words is judged like
/// prose; if it doesn't read as English, only its code-looking tokens
/// (`needless_borrow`, `src/main.rs`) are scored, not its words.
fn scored_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
    let mut terms = HashSet::new();
    for (is_code, part) in segments(text) {
        let words = part
            .split_whitespace()
            .filter_map(prose_word)
            .collect::<Vec<_>>();
        if !is_code || words.len() < MIN_WORDS_TO_JUDGE || english_share_ok(&words) {
            terms.extend(topic_terms(part, ignored));
        } else {
            for token in part
                .split_whitespace()
                .filter(|token| prose_word(token).is_none())
            {
                terms.extend(topic_terms(token, ignored));
            }
        }
    }
    terms
}

/// Punctuation that surrounds prose words rather than making up code:
/// anything but a letter, a digit or a character code is written with.
fn is_prose_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && !"-/\\_~$@#=+*<>|&%`^".contains(c)
}

/// Text with fewer words than this is too short to judge its language, and
/// is scored as it is (see [`prompt_reads_as_english`], [`scored_terms`]).
const MIN_WORDS_TO_JUDGE: usize = 4;

/// Whether the prompt reads as English, as memory turns must
/// ([`reads_as_english`]). A prompt with fewer than
/// [`MIN_WORDS_TO_JUDGE`] prose words *and* fewer than that many topic
/// words in all (`/analyze user login`) is too short to tell and passes.
/// Counting every topic word, code included, means a prompt that is mostly
/// code (``/fix `src/die/bin.rs` `mit_hat` `was_ist` ``) or a pasted error
/// (a German one in a fence, then `Hat man Ideen?`) is still judged.
///
/// Known limits: a longer English prompt with no function words (`/fix
/// flaky sqlite test timeout on linux ci`) gets no memories, failing
/// closed; a non-English prompt of three words or fewer is not checked.
fn prompt_reads_as_english(prompt: &str, ignored: &HashSet<String>) -> bool {
    let too_short_to_judge = prose_words(prompt).len() < MIN_WORDS_TO_JUDGE
        && topic_terms(prompt, ignored).len() < MIN_WORDS_TO_JUDGE;
    too_short_to_judge || reads_as_english(prompt)
}

/// Whether `text` reads as English: at least [`MIN_ENGLISH_MARKER_SHARE`]
/// of its [`prose_words`] are [`ENGLISH_MARKERS`] or English contractions.
/// Text with no prose outside code (`/fix` and a pasted error in a fence) is
/// judged on the prose-like words inside its code spans instead, the same
/// words [`scored_terms`] judges a span on.
///
/// The relevance filter only knows English. Another language's function
/// words (`schon`, `niet`, `jest`, `porque`) would be topic words to it, so
/// two unrelated sentences in that language could look relevant. Each
/// transcript turn of a memory is checked on its own, and only turns that
/// pass contribute topic words. A turn with too little English prose to
/// tell — a bare keyword list (`cargo fmt`) or a note that is almost all
/// code — fails the check too. Both fail closed.
///
/// Known limit: a word list screened against the languages named on
/// [`ENGLISH_MARKERS`] is not a language identifier. A turn in an
/// unscreened language that happens to use one of the markers, or a single
/// turn that mixes English with another language, is judged as English.
fn reads_as_english(text: &str) -> bool {
    let prose = prose_words(text);
    if !prose.is_empty() {
        return english_share_ok(&prose);
    }
    let code_words = segments(text)
        .into_iter()
        .filter(|(is_code, _)| *is_code)
        .flat_map(|(_, part)| part.split_whitespace().filter_map(prose_word))
        .collect::<Vec<_>>();
    english_share_ok(&code_words)
}

/// Whether at least [`MIN_ENGLISH_MARKER_SHARE`] of `words` are
/// [`ENGLISH_MARKERS`] or English contractions (and there are any).
fn english_share_ok(prose: &[String]) -> bool {
    let english = prose
        .iter()
        .filter(|word| {
            ENGLISH_MARKERS.contains(&word.as_str())
                || ENGLISH_CONTRACTION_ENDINGS
                    .iter()
                    .any(|ending| word.ends_with(ending))
        })
        .count();
    !prose.is_empty() && english as f64 / prose.len() as f64 >= MIN_ENGLISH_MARKER_SHARE
}

/// Distinct lower-cased topic words of `text`, without stop words, short
/// words or `ignored` words.
///
/// Only English is understood: stop words are the SMART list, and a word
/// with an apostrophe other than a possessive `'s` is a contraction
/// (`doesn't`, `they'll`, `you'd`) and never a topic. There is no stemming:
/// `tests` does not match `test`.
///
/// A topic word starts with a letter: numbers, with or without a suffix
/// (`500`, `2026`, `2nd`, `10am`, `100ms`), are shared by unrelated text too
/// often to count, while `sha256`, `utf8`, `e2e` and `x86` do. Terms that
/// start with a digit (`2fa`, `3des`) are dropped too, failing closed.
///
/// Known limit: only ASCII words are topic words. Accented Latin, Cyrillic,
/// Greek, and Chinese, Japanese and Korean text (which has no spaces
/// between words, or attaches grammar to them) contribute nothing, and
/// memory turns that do not read as English contribute nothing (see
/// [`reads_as_english`]), which also drops terse English notes with too few
/// function words to tell. A prompt that does not read as English gets no
/// memories at all (see [`prompt_reads_as_english`]). This fails closed: nothing irrelevant is
/// injected, a relevant memory may be missed.
fn topic_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
    words(text)
        .flatten()
        .filter(|word| {
            word.len() >= MIN_TERM_CHARS
                && word.chars().all(|c| c.is_ascii_alphanumeric())
                && word.starts_with(|c: char| c.is_ascii_alphabetic())
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
            "the cargo fmt failures detail",
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
                format_agent_memory_context(
                    &prompt.to_lowercase(),
                    &prompt_agents(prompt),
                    &memories
                ),
                "{prompt:?} is judged the same in either case"
            );
        }
        let none = HashSet::new();
        assert!(prompt_reads_as_english(
            "/FIX THE FLAKY SQLITE TEST ON THE LINUX RUNNER",
            &none
        ));
        assert!(prompt_reads_as_english(
            "/fix the HTTP API timeout in the auth client",
            &none
        ));
    }

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
        // Fences inside block quotes and list items are fences, and a
        // role-label line inside a closed fence doesn't split the turn.
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
        assert_eq!(
            turns("assistant: log:\n```\nx\n\nuser: y\n```\n\nuser: next"),
            ["log:\n```\nx\nuser: y\n```", "next"]
        );
        // A pasted `user: ```` line inside a fence is content, not a closer.
        assert_eq!(
            turns("assistant: log:\n```\nhallo\n\nuser: ```\nx\n```\n```\n\nuser: next"),
            ["log:\n```\nhallo\nuser: ```\nx\n```\n```", "next"]
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
        // Both readings must find the memory relevant: dropping each
        // reading's own words must not shrink the score's denominator.
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
        // Inside a fence, a diff, doc-comment or quote line that looks like
        // a fence in a container is content, not the fence's end.
        for fenced_prompt in [
            "/fix the markdown diff for the docs page:\n```diff\n- ```\n+ ~~~\n Fehler: die Datei hat man nicht gefunden, der Server ist weg\n```",
            "/fix the jsdoc example for this helper:\n```js\n/**\n * ```\n * Fehler: die Datei hat man nicht gefunden, der Server ist weg\n */\n```",
        ] {
            assert_eq!(
                format_agent_memory_context(
                    fenced_prompt,
                    &prompt_agents(fenced_prompt),
                    &man_memory
                ),
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
        // A pasted foreign error doesn't shrink a foreign prompt below the
        // size at which it is judged.
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
        // Known limit: a long English prompt without function words is
        // judged not English and gets nothing; a short one is not judged.
        let none = HashSet::new();
        assert!(!prompt_reads_as_english(
            "/fix flaky sqlite test timeout on linux ci",
            &none
        ));
        assert!(prompt_reads_as_english("/analyze user login", &none));
        assert!(prompt_reads_as_english("/fix the flaky sqlite test", &none));
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
            prompt_agents(
                "check ~/.amplihack/bin/amplihack-hook and /amplihack-recipe-runner output"
            )
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
        // `Use <name>.md agent` needs its capital `U`.
        assert!(prompt_agents("use my-reviewer.md agent to review").is_empty());
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
        let related = format!("{reference} why do the css grid columns break on the landing page");
        let result = format_agent_memory_context(&related, &agents(&["builder"]), &[memory(&css)])
            .expect("an on-topic prompt with the same reference still gets the memory");
        assert!(result.contains("tidied the css grid"));

        assert_eq!(
            without_definition_references(
                "use @~/.amplihack/.claude/agents/amplihack/core/builder.md to tidy, then read README.md"
            ),
            "use   to tidy, then read README.md"
        );
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
            "the cargo test ci detail",
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
