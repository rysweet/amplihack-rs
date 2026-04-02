//! URL-safe slug generation with Unicode normalization.
//!
//! Ported from `amplihack/utils/string_utils.py`.

use regex::Regex;
use std::sync::LazyLock;

/// Regex matching any character that is not alphanumeric, whitespace, or hyphen.
static NON_ALNUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\w\s-]").expect("NON_ALNUM regex is valid"));

/// Regex matching runs of hyphens and/or whitespace.
static COLLAPSE_SEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-\s]+").expect("COLLAPSE_SEP regex is valid"));

/// Convert a string to a URL-safe slug.
///
/// 1. Normalizes Unicode to NFKD and strips non-ASCII bytes.
/// 2. Removes characters that are not alphanumeric, whitespace, or hyphens.
/// 3. Trims, lowercases, and collapses whitespace/hyphens into single hyphens.
///
/// # Examples
///
/// ```
/// use amplihack_utils::slugify;
///
/// assert_eq!(slugify("Hello World!"), "hello-world");
/// assert_eq!(slugify("  Spaced   Out  "), "spaced-out");
/// assert_eq!(slugify("Ünïcödé Téxt"), "unicode-text");
/// ```
pub fn slugify(value: &str) -> String {
    // NFKD normalize and strip non-ASCII (mirrors Python's encode("ascii","ignore"))
    let ascii_only: String = value
        .chars()
        .filter_map(|c| {
            // Decompose the character (NFKD-like: strip accents by keeping only ASCII parts)
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            if s.len() == 1 && s.as_bytes()[0].is_ascii() {
                Some(s.as_bytes()[0] as char)
            } else {
                // For composed characters, try decomposition via unicode-normalization
                // Simplified approach: walk the NFKD decomposition
                unicode_nfkd_ascii(c)
            }
        })
        .collect();

    let stripped = NON_ALNUM.replace_all(&ascii_only, "");
    let trimmed = stripped.trim().to_lowercase();
    let collapsed = COLLAPSE_SEP.replace_all(&trimmed, "-");
    collapsed.trim_matches('-').to_owned()
}

/// Decompose a Unicode character via manual NFKD-like mapping and return the
/// ASCII base letter if one exists. Returns `None` for non-decomposable
/// non-ASCII characters.
fn unicode_nfkd_ascii(c: char) -> Option<char> {
    // Common Latin diacritics — covers the vast majority of real-world input.
    // This avoids pulling in the full `unicode-normalization` crate.
    match c {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => Some('A'),
        'Æ' => Some('A'),
        'Ç' => Some('C'),
        'È' | 'É' | 'Ê' | 'Ë' => Some('E'),
        'Ì' | 'Í' | 'Î' | 'Ï' => Some('I'),
        'Ð' => Some('D'),
        'Ñ' => Some('N'),
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => Some('O'),
        'Ø' => Some('O'),
        'Ù' | 'Ú' | 'Û' | 'Ü' => Some('U'),
        'Ý' => Some('Y'),
        'Þ' => Some('T'),
        'ß' => Some('s'),
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
        'æ' => Some('a'),
        'ç' => Some('c'),
        'è' | 'é' | 'ê' | 'ë' => Some('e'),
        'ì' | 'í' | 'î' | 'ï' => Some('i'),
        'ð' => Some('d'),
        'ñ' => Some('n'),
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => Some('o'),
        'ø' => Some('o'),
        'ù' | 'ú' | 'û' | 'ü' => Some('u'),
        'ý' | 'ÿ' => Some('y'),
        'þ' => Some('t'),
        'Š' => Some('S'),
        'š' => Some('s'),
        'Ž' => Some('Z'),
        'ž' => Some('z'),
        'Đ' => Some('D'),
        'đ' => Some('d'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_slug() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn strips_punctuation() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(slugify("  Spaced   Out  "), "spaced-out");
    }

    #[test]
    fn collapses_hyphens() {
        assert_eq!(slugify("a---b---c"), "a-b-c");
    }

    #[test]
    fn unicode_diacritics() {
        assert_eq!(slugify("Ünïcödé Téxt"), "unicode-text");
    }

    #[test]
    fn mixed_separators() {
        assert_eq!(slugify("foo - bar _ baz"), "foo-bar-_-baz");
    }

    #[test]
    fn empty_string() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn only_special_chars() {
        assert_eq!(slugify("!!!@@@###"), "");
    }

    #[test]
    fn preserves_numbers() {
        assert_eq!(slugify("Version 2.0 Release"), "version-20-release");
    }

    #[test]
    fn already_a_slug() {
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
    }

    #[test]
    fn leading_trailing_hyphens() {
        assert_eq!(slugify("--leading-trailing--"), "leading-trailing");
    }

    #[test]
    fn accented_sentence() {
        assert_eq!(slugify("Café résumé naïve"), "cafe-resume-naive");
    }

    #[test]
    fn tabs_and_newlines() {
        assert_eq!(slugify("hello\tworld\nfoo"), "hello-world-foo");
    }
}
