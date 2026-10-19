//! Tokenising and segmenting memory and prompt text: words, prose and
//! code parts, and transcript turns.

/// The words of `text`: split on anything but letters, digits and
/// apostrophes, with contractions (`None` from [`without_apostrophes`])
/// kept as `None`.
pub(super) fn words(text: &str) -> impl Iterator<Item = Option<&str>> {
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
pub(super) fn segments(text: &str) -> Vec<(bool, &str)> {
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

/// `text` split at fenced code blocks (CommonMark §4.5): a line that
/// starts, after indentation, with three or more backticks or tildes opens
/// a fence (a backtick fence's info string may not contain a backtick), and
/// the next line holding at least as many of the same character and nothing
/// else closes it. Everything between is code. Fences inside block quotes
/// or list items are not recognised; their text stays prose.
fn fenced_blocks(text: &str) -> Vec<(bool, &str)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut parts = Vec::new();
    let mut prose_start = 0;
    let mut index = 0;
    while index < lines.len() {
        let (open_at, open_line) = lines[index];
        let close = fence_run(open_line)
            .filter(|(marker, _, rest)| !(*marker == '`' && rest.contains('`')))
            .and_then(|(marker, width, _)| {
                lines[index + 1..].iter().position(|(_, line)| {
                    fence_run(line).is_some_and(|(closer, closer_width, rest)| {
                        closer == marker && closer_width >= width && rest.trim().is_empty()
                    })
                })
            });
        let Some(close) = close else {
            index += 1;
            continue;
        };
        let (close_at, close_line) = lines[index + 1 + close];
        parts.push((false, &text[prose_start..open_at]));
        parts.push((true, &text[open_at + open_line.len()..close_at]));
        prose_start = close_at + close_line.len();
        index += close + 2;
    }
    parts.push((false, &text[prose_start..]));
    parts
}

/// The fence run `line` starts with after indentation, if any: its
/// character (`` ` `` or `~`), its length (at least 3) and the rest of the
/// line.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let line = line.trim_start();
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let width = line.chars().take_while(|c| *c == marker).count();
    (width >= 3).then(|| (marker, width, &line[width..]))
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

/// `text` split into transcript turns, without their role labels.
///
/// Session-stop flattens a transcript as `<role>: <text>` paragraphs joined
/// by blank lines, with whatever role the transcript carries. A paragraph
/// that starts with one of the [`TRANSCRIPT_ROLES`] labels starts a turn,
/// even inside a code block: the flattened text can't tell a pasted log
/// line from a real turn, and a split never merges two turns' languages.
/// Other paragraphs (a code block with blank lines in it, or a note that
/// opens with `sqlite: …`) continue the current turn. Text without labels
/// is one turn.
pub(super) fn turns(text: &str) -> Vec<String> {
    let mut turns = vec![String::new()];
    let mut paragraph_start = true;
    for line in text.lines() {
        if line.trim().is_empty() {
            paragraph_start = true;
            continue;
        }
        let label = line
            .trim_start()
            .split_once(": ")
            .filter(|(role, _)| paragraph_start && TRANSCRIPT_ROLES.contains(role));
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
pub(super) fn prose_words(text: &str) -> Vec<String> {
    segments(text)
        .into_iter()
        .filter(|(is_code, _)| !is_code)
        .flat_map(|(_, part)| part.split_whitespace().filter_map(prose_word))
        .collect()
}

/// `token` as a lower-cased prose word, or `None` when it looks like code.
/// Capitals alone don't make code: `ICH BIN` is still words that
/// `topic_terms` scores, so the language check must see them too.
pub(super) fn prose_word(token: &str) -> Option<String> {
    let token = token
        .trim_start_matches(|c: char| is_prose_punctuation(c) && c != '-')
        .trim_end_matches(is_prose_punctuation);
    let looks_like_code = token
        .chars()
        .any(|c| !c.is_alphabetic() && !is_apostrophe(c));
    (!token.is_empty() && !looks_like_code).then(|| token.to_lowercase().replace('\u{2019}', "'"))
}

/// Punctuation that surrounds prose words rather than making up code:
/// anything but a letter, a digit or a character code is written with.
fn is_prose_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && !"-/\\_~$@#=+*<>|&%`^".contains(c)
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// `raw` with surrounding quotes and a possessive `'s` dropped
/// (`builder's` → `builder`), or `None` when an apostrophe remains: the
/// word is a contraction (`doesn't`, `we'll`, `you'd`), which carries no
/// topic, and whose apostrophe-less spelling can collide with a real word
/// (`we'll` → `well`).
pub(super) fn without_apostrophes(raw: &str) -> Option<&str> {
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
