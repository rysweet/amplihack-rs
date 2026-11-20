//! Issue #2589 (TDD), SEC-V1/SEC-V2: the `Prefix` newtype.
//!
//! Expected API, in `policy/prefix.rs`:
//!
//! ```ignore
//! pub(crate) struct Prefix(String);
//! impl Prefix {
//!     pub(crate) const MAX_BYTES: usize = 128;
//!     pub(crate) const MAX_TOKENS: usize = 8;
//!     pub(crate) fn as_str(&self) -> &str;
//! }
//! impl TryFrom<&str> for Prefix { type Error = PrefixError; }
//! pub(crate) enum PrefixError { ... }   // Display, one plain sentence
//! ```
//!
//! The constructor is the only way to make a `Prefix`, so a renderer can
//! never be handed an unvalidated string. Validation is an ASCII whitelist,
//! not escaping: a blacklist of quote and backslash would leave the closing
//! parenthesis, the colon and the glob characters live, and each of those is
//! structural in at least one of the three renderings.

use super::prefix::{MAX_BYTES, MAX_TOKENS, Prefix};

fn ok(raw: &str) -> Prefix {
    Prefix::try_from(raw).unwrap_or_else(|e| panic!("{raw:?} should be accepted: {e}"))
}

#[test]
fn accepts_the_shapes_the_default_list_needs() {
    for raw in [
        "pkill",
        "killall",
        "pgrep",
        "tmux kill-server",
        "tmux kill-session -a",
        "git push --force",
        "git push --force-with-lease",
        "git push -d",
    ] {
        assert_eq!(ok(raw).as_str(), raw);
    }
}

#[test]
fn accepts_the_whitelisted_characters_only() {
    // [A-Za-z0-9._/@=+-] and single spaces between tokens.
    assert_eq!(ok("/usr/bin/pkill").as_str(), "/usr/bin/pkill");
    assert_eq!(ok("a.b_c-d=e+f@g").as_str(), "a.b_c-d=e+f@g");
    assert_eq!(ok("one two three").as_str(), "one two three");
}

#[test]
fn rejects_every_character_that_is_syntax_in_a_rendering() {
    // `:` and `*` end a Claude prefix; `(`, `)` delimit a Copilot tool
    // pattern; the quotes and the backslash are shell and Starlark syntax;
    // the rest are shell metacharacters.
    for bad in [
        "a:b", "a*b", "a?b", "a(b", "a)b", "a'b", "a\"b", "a\\b", "a$b", "a;b", "a|b", "a&b",
        "a>b", "a<b", "a`b", "a{b", "a}b", "a[b", "a]b", "a#b", "a!b", "a~b", "a,b", "a%b", "a^b",
    ] {
        assert!(
            Prefix::try_from(bad).is_err(),
            "{bad:?} must be rejected by the whitelist"
        );
    }
}

#[test]
fn rejects_control_characters_nul_and_newlines() {
    for bad in ["a\tb", "a\nb", "a\rb", "a\u{0}b", "a\u{7f}b", "a\u{1b}b"] {
        assert!(Prefix::try_from(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn rejects_non_ascii_including_bidi_and_lookalikes() {
    for bad in ["pkíll", "pkill\u{202e}", "pkill\u{a0}-f", "кill"] {
        assert!(Prefix::try_from(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn rejects_empty_leading_trailing_and_repeated_separators() {
    for bad in [
        "",
        " ",
        "pkill ",
        " pkill",
        "git  push",
        "git push  --force",
    ] {
        assert!(
            Prefix::try_from(bad).is_err(),
            "{bad:?} must be rejected: separators are single spaces between tokens"
        );
    }
}

#[test]
fn rejects_a_value_over_the_byte_limit() {
    let long = "a".repeat(MAX_BYTES);
    assert_eq!(ok(&long).as_str().len(), MAX_BYTES);
    assert!(Prefix::try_from("a".repeat(MAX_BYTES + 1).as_str()).is_err());
}

#[test]
fn rejects_a_value_over_the_token_limit() {
    let at_limit = ["a"; MAX_TOKENS].join(" ");
    assert!(Prefix::try_from(at_limit.as_str()).is_ok());
    let over = ["a"; MAX_TOKENS + 1].join(" ");
    assert!(Prefix::try_from(over.as_str()).is_err());
}

#[test]
fn the_error_is_one_plain_sentence_with_no_raw_value_echoed() {
    let err = Prefix::try_from("shell(x)").unwrap_err();
    let text = err.to_string();
    assert!(!text.is_empty());
    assert!(!text.contains('\n'), "{text}");
    assert!(
        !text.contains('\u{1b}'),
        "a reason never carries a control character: {text}"
    );
}

#[test]
fn prefixes_compare_and_hash_by_their_text_so_duplicates_collapse() {
    use std::collections::HashSet;
    let set: HashSet<Prefix> = ["pkill", "pkill", "killall"]
        .iter()
        .map(|raw| ok(raw))
        .collect();
    assert_eq!(set.len(), 2);
    assert_eq!(ok("pkill"), ok("pkill"));
    assert_ne!(ok("pkill"), ok("killall"));
}
