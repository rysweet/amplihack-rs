//! The fail-closed check that a memory turn reads as English.

use super::text::{prose_word, prose_words, segments};

/// A memory turn contributes topic words only if at least this share of
/// its prose words are [`ENGLISH_MARKERS`] or English contractions, i.e. it
/// reads as English (see [`reads_as_english`]).
const MIN_ENGLISH_MARKER_SHARE: f64 = 0.1;

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

/// Whether `text` reads as English: at least [`MIN_ENGLISH_MARKER_SHARE`]
/// of its [`prose_words`] are [`ENGLISH_MARKERS`] or English contractions.
/// Text with no prose outside code (an assistant reply that is only a
/// fenced block) is judged on the prose-like words inside its code instead.
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
pub(super) fn reads_as_english(text: &str) -> bool {
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
