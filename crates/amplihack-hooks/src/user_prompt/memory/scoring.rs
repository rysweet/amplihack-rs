//! Topic words and relevance: stop words, the foreign-code check, topic
//! words and the cosine score of a memory against the prompt.

use super::text::{prose_word, segments, words};
use crate::agent_memory::slash_commands_for;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Minimum relevance a memory needs before it is injected into the prompt.
pub(super) const RELEVANCE_THRESHOLD: f64 = 0.2;
/// A memory must also share at least this many topic words with the prompt,
/// so one coincidental word never makes a short memory relevant.
///
/// Known limit, chosen deliberately: a prompt left with a single topic word
/// once its agent names and slash command are ignored (`/fix clippy`) never
/// matches. Letting one word suffice re-admits exactly the #1483 failure
/// (`/improve reply to the reviewer` pulling in the stored `reply with just:
/// pong` smoke test), and one shared word is not evidence of relevance.
pub(super) const MIN_SHARED_TERMS: usize = 2;
/// Topic words shorter than this (`rs`, `md`, `ci`) are too common to count.
const MIN_TERM_CHARS: usize = 3;

/// The SMART information-retrieval system's English stop list (Salton,
/// Cornell; 571 words), verbatim from
/// <https://github.com/igorbrigadir/stopwords/blob/master/en/smart.txt>.
/// A standard list rather than a hand-grown one: any two function words
/// missing from a short list would make a chit-chat transcript "relevant".
const SMART_STOP_WORDS: &str = include_str!("../smart_stop_words.txt");

/// Words that carry no topic here beyond the SMART list: the words of the
/// `Agent <name>:` prefix every stored learning carries.
const EXTRA_STOP_WORDS: &[&str] = &["agent", "agents", "general"];

/// Frequent function words of German, Spanish, French, Italian, Portuguese
/// and Dutch (from their stopwords-iso lists,
/// <https://github.com/stopwords-iso/stopwords-iso>) that are also English
/// words, so a prompt in one of those languages can't match an English
/// memory on them (`ich bin nicht sicher, warum die Tests scheitern` against
/// `the workers die if the bin directory is missing`). The prompt is not
/// language-checked: that would drop terse English prompts too (`/fix flaky
/// sqlite test timeout on linux ci`), which have no function words to judge.
///
/// Every entry is pinned by a test
/// (`foreign_function_words_are_stop_words` and the non-English prompt
/// tests), and the cost is that a two-word prompt made of an entry and one
/// other topic word never matches (`/fix the man page`, see
/// [`MIN_SHARED_TERMS`]). Left out for that reason, as everyday developer
/// vocabulary: `bin`, `dos`, `dir`, `due`, `sin`, `col`, `del`, `com`,
/// `net`; `die` alone keeps the German `bin … die` prompts out. `man` is
/// kept because German `hat man` in a pasted log otherwise matches an
/// English memory about a man and his hat.
const FOREIGN_FUNCTION_WORDS: &[&str] = &[
    "car", "con", "den", "die", "doe", "era", "hat", "man", "met", "mit", "sea", "son", "ton",
    "van", "war",
];

/// Function words of the same six languages (stopwords-iso) that are not
/// English words, shell words, acronyms or names: positive evidence that a
/// code part of a memory (a pasted log) is in another language (see
/// [`scored_terms`]). A command line has none of them. They are evidence
/// only, never stop words, so a wrong entry costs recall on every fence
/// that happens to contain it; [`prose_word`] lower-cases, so an acronym or
/// a name is a hit too. Screened out for that reason: `aux` (`ps aux`),
/// `pour` (English verb), `el` (`const el`, El Paso), `est` (EST,
/// estimate), `des` (DES), `il` (Illinois), `las`, `los` (Las Vegas, Los
/// Angeles), `para` (paragraph, `para-`), `pas` (pas de deux), `ist` (IST,
/// a time zone), `das` (DAS, storage), `dem` (DEM, elevation data), `les`,
/// `una`, `che`, `della`, `nella`, `uma` (names), `que` (SMART), `non`
/// (English prefix), `com` (domains). German, Dutch and French keep enough
/// entries to carry a paste on their own; Italian keeps two.
pub(super) const FOREIGN_EVIDENCE: &[&str] = &[
    // German
    "auch", "der", "eine", "ich", "keine", "nicht", "noch", "sind", "und", "wie", "wird", "wo",
    "zum", "zur", // Spanish
    "está", "están", "pero", "por", // French
    "avec", "cette", "dans", "le", "sur", "une", // Italian
    "gli", "sono", // Portuguese
    "não", "são", "também", // Dutch
    "een", "het", "naar", "niet", "ook", "zijn",
];

/// A code part is foreign on at least this much evidence, counted per code
/// part (a paste is one fence): two listed words, or one listed word and a
/// word with a non-ASCII letter. Accented words alone (`Jürgen Müller` in
/// a git log) are not evidence, so a paste in a diacritic language outside
/// the six (Polish, Czech, Turkish) is never demoted either.
const MIN_FOREIGN_EVIDENCE: usize = 2;

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
                .chain(FOREIGN_FUNCTION_WORDS.iter().copied())
                .collect()
        })
        .contains(word)
}

/// The agent names, and the slash commands that invoke them, are how the
/// prompt reached this hook, not what it is about.
pub(super) fn ignored_terms(agent_types: &[String]) -> HashSet<String> {
    let mut ignored = HashSet::new();
    for agent in agent_types {
        ignored.extend(topic_terms(agent, &HashSet::new()));
        for command in slash_commands_for(agent) {
            ignored.insert(command.to_string());
        }
    }
    ignored
}

/// The topic words of a memory turn that are scored: those of its prose,
/// and of each code part unless that part shows it is in another language.
/// Code is left out of the language check, so a German log pasted in a
/// fence or a backtick span by an English assistant turn would otherwise
/// lend its words to a German prompt. A code part is judged foreign only on
/// positive evidence, [`MIN_FOREIGN_EVIDENCE`] hits among its prose-like
/// words: words from [`FOREIGN_EVIDENCE`], of which at least one is needed,
/// and words with a non-ASCII letter (`größer`, `não`). A command line has
/// neither, so `cargo run ingest parquet crash repro` and `ps aux | grep
/// ingest worker` keep their words, while a foreign part contributes only
/// its code-looking tokens (`needless_borrow`, `src/main.rs`). Absence of
/// English function words is not evidence: a command has none either.
pub(super) fn scored_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
    let mut terms = HashSet::new();
    for (is_code, part) in segments(text) {
        let (mut listed, mut non_ascii) = (0, 0);
        if is_code {
            for word in part.split_whitespace().filter_map(prose_word) {
                if FOREIGN_EVIDENCE.contains(&word.as_str()) {
                    listed += 1;
                } else if !word.is_ascii() {
                    non_ascii += 1;
                }
            }
        }
        let foreign = listed >= 1 && listed + non_ascii >= MIN_FOREIGN_EVIDENCE;
        if !foreign {
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
/// `reads_as_english`), which also drops terse English notes with too few
/// function words to tell. This fails closed: nothing irrelevant is
/// injected, a relevant memory may be missed.
pub(super) fn topic_terms(text: &str, ignored: &HashSet<String>) -> HashSet<String> {
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

/// Cosine similarity of the prompt's and the memory's topic-word sets, in
/// `[0, 1]`. Dividing by both set sizes keeps a long transcript, which shares
/// some words with almost any prompt, from scoring as relevant.
pub(super) fn memory_relevance(
    prompt_terms: &HashSet<String>,
    memory_terms: &HashSet<String>,
) -> f64 {
    if prompt_terms.is_empty() || memory_terms.is_empty() {
        return 0.0;
    }
    let shared = prompt_terms.intersection(memory_terms).count() as f64;
    shared / ((prompt_terms.len() * memory_terms.len()) as f64).sqrt()
}
