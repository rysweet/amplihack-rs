//! Issue #1483 / PR #1487: the shape of the `user_prompt` memory module.
//!
//! `user_prompt/memory.rs` grew to 2413 lines while the relevance gate was
//! hardened over five review rounds. The 39 unit tests already pin what it
//! does. This file pins how it is laid out, so that the split into
//! `memory.rs` plus `memory/{text,language,scoring}.rs` stays in place:
//!
//! * every production file is under the 400-line brick budget;
//! * each item is defined in exactly one file, the one the reference doc
//!   (`docs/reference/user-prompt-submit-hook-api.md`) names;
//! * dependencies point one way (`text` has none; `language` and `scoring`
//!   use only `text`), so no file can grow back into the others;
//! * the public surface is still exactly `inject_memory`,
//!   `format_agent_memory_context` and `check_framework_injection`, and
//!   items that cross a file boundary use `pub(super)`, nothing wider;
//! * none of the 39 tests is lost in the move.
//!
//! It reads the source as text, the same way
//! `amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs` does.

use std::fs;
use std::path::{Path, PathBuf};

/// The brick budget: a production file must be shorter than this.
const BRICK_LIMIT: usize = 400;

/// Test-only files are not budgeted, but they must stay readable.
const TEST_FILE_LIMIT: usize = 600;

fn user_prompt_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/user_prompt")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Production files of the memory module, relative to `src/user_prompt`.
const PRODUCTION_FILES: &[&str] = &[
    "memory.rs",
    "memory/text.rs",
    "memory/language.rs",
    "memory/scoring.rs",
];

fn read(rel: &str) -> String {
    let path = user_prompt_dir().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read src/user_prompt/{rel} ({err}); the #1483 split puts \
             the memory module in memory.rs plus memory/{{text,language,scoring}}.rs"
        )
    })
}

/// Source with the `#[cfg(test)] mod tests { ... }` block, if any, removed.
/// After the split there must be none, but stripping keeps a failure in one
/// test from cascading into the others.
fn production_source(rel: &str) -> String {
    let src = read(rel);
    match src.find("#[cfg(test)]\nmod tests {") {
        Some(at) => src[..at].to_string(),
        None => src,
    }
}

/// Every `.rs` file under `memory/tests` plus `memory/tests.rs`.
fn test_files() -> Vec<(String, String)> {
    let mut files = vec![("memory/tests.rs".to_string(), read("memory/tests.rs"))];
    let dir = user_prompt_dir().join("memory/tests");
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("cannot read {} ({err})", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    entries.sort();
    for path in entries {
        let rel = format!(
            "memory/tests/{}",
            path.file_name().unwrap().to_string_lossy()
        );
        let body = fs::read_to_string(&path).expect("test file is readable");
        files.push((rel, body));
    }
    files
}

/// Whether `src` defines `kind name` at the start of a line, with any
/// visibility in front (`fn foo`, `pub(super) fn foo`, `const FOO:`).
fn defines(src: &str, kind: &str, name: &str) -> bool {
    src.lines().any(|line| {
        let line = line.trim_start();
        let line = line
            .strip_prefix("pub(super) ")
            .or_else(|| line.strip_prefix("pub(crate) "))
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        let Some(rest) = line.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')) else {
            return false;
        };
        rest.strip_prefix(name)
            .is_some_and(|after| after.starts_with(['(', ':', '<', ' ']))
    })
}

#[test]
fn production_files_are_under_the_brick_budget() {
    let mut over = Vec::new();
    for rel in PRODUCTION_FILES {
        let lines = read(rel).lines().count();
        if lines >= BRICK_LIMIT {
            over.push(format!("src/user_prompt/{rel}: {lines} lines"));
        }
    }
    assert!(
        over.is_empty(),
        "production files must be under {BRICK_LIMIT} lines; split further, \
         do not compress:\n{}",
        over.join("\n")
    );
}

#[test]
fn test_files_stay_readable() {
    let over: Vec<_> = test_files()
        .into_iter()
        .map(|(rel, body)| (rel, body.lines().count()))
        .filter(|(_, lines)| *lines > TEST_FILE_LIMIT)
        .map(|(rel, lines)| format!("src/user_prompt/{rel}: {lines} lines"))
        .collect();
    assert!(
        over.is_empty(),
        "split test files by area once they pass {TEST_FILE_LIMIT} lines \
         (for example memory/tests/code_spans.rs):\n{}",
        over.join("\n")
    );
}

#[test]
fn memory_root_declares_its_child_modules() {
    let src = read("memory.rs");
    for decl in ["mod text;", "mod language;", "mod scoring;"] {
        assert!(
            src.lines().any(|line| line.trim() == decl),
            "memory.rs must declare `{decl}`"
        );
    }
    assert!(
        src.contains("#[cfg(test)]\nmod tests;"),
        "memory.rs must load its tests from memory/tests.rs with \
         `#[cfg(test)]\\nmod tests;`, not an inline `mod tests {{ ... }}` block"
    );
}

/// Where each item lives after the split: `(file, kind, name)`.
const PLACEMENT: &[(&str, &str, &str)] = &[
    // Root: entry points, agent detection and output limits.
    ("memory.rs", "const", "MAX_INJECTED_MEMORIES"),
    ("memory.rs", "const", "MAX_MEMORY_CHARS"),
    ("memory.rs", "fn", "prompt_agents"),
    ("memory.rs", "fn", "inject_memory"),
    ("memory.rs", "fn", "format_agent_memory_context"),
    ("memory.rs", "fn", "strip_agent_prefix"),
    ("memory.rs", "fn", "single_line"),
    ("memory.rs", "fn", "bounded_memory_text"),
    ("memory.rs", "fn", "generate_unique_session_id"),
    ("memory.rs", "fn", "check_framework_injection"),
    ("memory.rs", "fn", "find_amplihack_md"),
    // Text: tokenising and segmenting.
    ("memory/text.rs", "fn", "words"),
    ("memory/text.rs", "fn", "segments"),
    ("memory/text.rs", "fn", "fenced_blocks"),
    ("memory/text.rs", "fn", "fence_run"),
    ("memory/text.rs", "fn", "backtick_spans"),
    ("memory/text.rs", "const", "TRANSCRIPT_ROLES"),
    ("memory/text.rs", "fn", "turns"),
    ("memory/text.rs", "fn", "prose_words"),
    ("memory/text.rs", "fn", "prose_word"),
    ("memory/text.rs", "fn", "is_prose_punctuation"),
    ("memory/text.rs", "fn", "is_apostrophe"),
    ("memory/text.rs", "fn", "without_apostrophes"),
    // Language: the fail-closed English check.
    ("memory/language.rs", "const", "ENGLISH_MARKERS"),
    ("memory/language.rs", "const", "ENGLISH_CONTRACTION_ENDINGS"),
    ("memory/language.rs", "const", "MIN_ENGLISH_MARKER_SHARE"),
    ("memory/language.rs", "fn", "reads_as_english"),
    ("memory/language.rs", "fn", "english_share_ok"),
    // Scoring: stop words, foreign-code check, topic words, relevance.
    ("memory/scoring.rs", "const", "RELEVANCE_THRESHOLD"),
    ("memory/scoring.rs", "const", "MIN_SHARED_TERMS"),
    ("memory/scoring.rs", "const", "MIN_TERM_CHARS"),
    ("memory/scoring.rs", "const", "SMART_STOP_WORDS"),
    ("memory/scoring.rs", "const", "EXTRA_STOP_WORDS"),
    ("memory/scoring.rs", "const", "FOREIGN_FUNCTION_WORDS"),
    ("memory/scoring.rs", "const", "FOREIGN_EVIDENCE"),
    ("memory/scoring.rs", "const", "MIN_FOREIGN_EVIDENCE"),
    ("memory/scoring.rs", "const", "DEV_VOCABULARY"),
    ("memory/scoring.rs", "fn", "is_stop_word"),
    ("memory/scoring.rs", "fn", "ignored_terms"),
    ("memory/scoring.rs", "fn", "topic_terms"),
    ("memory/scoring.rs", "fn", "scored_terms"),
    ("memory/scoring.rs", "fn", "memory_relevance"),
];

#[test]
fn each_item_is_defined_once_in_its_own_file() {
    let sources: Vec<_> = PRODUCTION_FILES
        .iter()
        .map(|rel| (*rel, production_source(rel)))
        .collect();
    let mut wrong = Vec::new();
    for (home, kind, name) in PLACEMENT {
        let found: Vec<_> = sources
            .iter()
            .filter(|(_, src)| defines(src, kind, name))
            .map(|(rel, _)| *rel)
            .collect();
        if found != [*home] {
            wrong.push(format!(
                "{kind} {name}: expected in {home}, found in {found:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "misplaced items:\n{}", wrong.join("\n"));
}

/// Whether `src` refers to the sibling module `name` (`super::name`,
/// `use super::{name, ...}` or `super::{..., name::...}`).
fn uses_sibling(src: &str, name: &str) -> bool {
    src.contains(&format!("super::{name}"))
        || src.lines().any(|line| {
            let line = line.trim();
            line.starts_with("use super::{")
                && line
                    .trim_start_matches("use super::{")
                    .split([',', '}', ' '])
                    .any(|part| part == name || part.starts_with(&format!("{name}::")))
        })
}

#[test]
fn dependencies_point_one_way() {
    let text = production_source("memory/text.rs");
    let language = production_source("memory/language.rs");
    let scoring = production_source("memory/scoring.rs");

    for sibling in ["language", "scoring"] {
        assert!(
            !uses_sibling(&text, sibling),
            "memory/text.rs must not depend on `{sibling}`; text has no dependencies"
        );
    }
    assert!(
        !uses_sibling(&language, "scoring"),
        "memory/language.rs must not depend on `scoring`"
    );
    assert!(
        !uses_sibling(&scoring, "language"),
        "memory/scoring.rs must not depend on `language`"
    );
    for (rel, src) in [
        ("memory/language.rs", &language),
        ("memory/scoring.rs", &scoring),
    ] {
        assert!(
            uses_sibling(src, "text"),
            "{rel} is expected to build on memory/text.rs"
        );
    }
    // Children must not reach back into the root's private items.
    for (rel, src) in [
        ("memory/text.rs", &text),
        ("memory/language.rs", &language),
        ("memory/scoring.rs", &scoring),
    ] {
        for root_item in [
            "format_agent_memory_context",
            "inject_memory",
            "MAX_MEMORY_CHARS",
        ] {
            assert!(
                !src.contains(root_item),
                "{rel} must not use the root item `{root_item}`"
            );
        }
    }
}

/// Lines declaring an item with `pub` or `pub(crate)` visibility.
fn wide_items(src: &str) -> Vec<String> {
    src.lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("pub ") || line.starts_with("pub(crate) "))
        .map(str::to_string)
        .collect()
}

#[test]
fn public_surface_is_unchanged() {
    let root = wide_items(&production_source("memory.rs"));
    let expected = [
        "pub(crate) fn inject_memory(",
        "pub fn format_agent_memory_context(",
        "pub(crate) fn check_framework_injection(",
    ];
    for item in expected {
        assert!(
            root.iter().any(|line| line.starts_with(item)),
            "memory.rs must still declare `{item}...`; found {root:?}"
        );
    }
    let extra: Vec<_> = root
        .iter()
        .filter(|line| {
            !line.starts_with("pub use") && !expected.iter().any(|e| line.starts_with(e))
        })
        .collect();
    assert!(
        extra.is_empty(),
        "memory.rs widened its public surface: {extra:?}"
    );

    for rel in ["memory/text.rs", "memory/language.rs", "memory/scoring.rs"] {
        let wide = wide_items(&production_source(rel));
        assert!(
            wide.is_empty(),
            "{rel} may use `pub(super)` at most, found: {wide:?}"
        );
    }
}

#[test]
fn production_files_hide_nothing_from_the_linter() {
    for rel in PRODUCTION_FILES {
        let src = production_source(rel);
        for banned in [
            "#[allow(dead_code)]",
            "#![allow(dead_code)]",
            "#[allow(unused",
        ] {
            assert!(
                !src.contains(banned),
                "{rel} uses `{banned}`; fix visibility with pub(super) instead"
            );
        }
        for banned in ["dbg!(", "println!(", "eprintln!("] {
            assert!(!src.contains(banned), "{rel} must not print (`{banned}`)");
        }
    }
}

/// The 39 tests that pin #1483's behaviour. The split moves them; it must
/// not drop, rename or duplicate any.
const PINNED_TESTS: &[&str] = &[
    "relevant_memory_is_injected_with_its_computed_score",
    "irrelevant_memory_injects_nothing",
    "no_memories_injects_nothing",
    "each_memory_is_printed_once_whatever_the_agent_count",
    "per_agent_prefixed_copies_are_printed_once_without_prefix",
    "one_shared_word_is_not_relevant",
    "single_topic_word_prompt_does_not_match",
    "memories_differing_past_the_cut_are_both_kept",
    "whitespace_variant_copies_are_printed_once",
    "japanese_and_korean_grammar_does_not_make_memories_relevant",
    "english_function_words_do_not_make_memories_relevant",
    "chinese_function_characters_do_not_make_memories_relevant",
    "contractions_do_not_make_memories_relevant",
    "non_english_memories_are_not_injected",
    "other_languages_function_words_do_not_make_memories_relevant",
    "developer_vocabulary_is_matched",
    "language_check_counts_english_function_words",
    "technical_english_memories_are_injected",
    "english_turns_and_unclosed_spans_are_read",
    "capitalised_foreign_text_is_judged",
    "foreign_text_in_code_spans_is_judged",
    "non_english_prompts_are_not_scored",
    "quoted_commands_keep_their_words",
    "foreign_prompts_do_not_match_foreign_text_pasted_in_english_memories",
    "foreign_function_words_are_stop_words",
    "developer_words_are_not_foreign_stop_words",
    "terse_english_prompts_get_their_memories",
    "numbers_are_not_topic_words",
    "terse_notes_without_english_function_words_are_not_scored",
    "only_slash_words_name_agents",
    "definition_reference_paths_are_not_topic_words",
    "empty_agent_list_has_a_plain_heading",
    "agent_prefix_is_stripped_only_when_it_is_one",
    "long_transcript_is_not_relevant_just_by_sharing_words",
    "two_shared_words_below_the_threshold_are_not_relevant",
    "injected_memory_is_bounded",
    "at_most_max_memories_are_injected_most_relevant_first",
    "format_with_code_context",
    "format_empty_code_context_skipped",
];

/// Names of `#[test]` functions in `src`.
fn test_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = false;
    for line in src.lines().map(str::trim) {
        if line == "#[test]" {
            pending = true;
        } else if pending {
            if let Some(rest) = line.strip_prefix("fn ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                names.push(name);
                pending = false;
            } else if !line.starts_with("#[") {
                pending = false;
            }
        }
    }
    names
}

#[test]
fn every_pinned_test_moved_exactly_once() {
    let mut found: Vec<String> = test_files()
        .iter()
        .flat_map(|(_, body)| test_names(body))
        .collect();
    found.sort();

    let mut missing = Vec::new();
    for name in PINNED_TESTS {
        match found.iter().filter(|f| f == name).count() {
            1 => {}
            0 => missing.push(format!("{name}: missing")),
            n => missing.push(format!("{name}: defined {n} times")),
        }
    }
    assert!(
        missing.is_empty(),
        "memory/tests/ must hold each of the {} pinned tests once:\n{}",
        PINNED_TESTS.len(),
        missing.join("\n")
    );
    assert_eq!(
        found.len(),
        PINNED_TESTS.len(),
        "memory/tests/ holds tests that are not in PINNED_TESTS; add them \
         there so the count stays checked: {found:?}"
    );
}

#[test]
fn every_test_file_is_declared() {
    // An undeclared area file is never compiled, so its tests silently stop running.
    let root = read("memory/tests.rs");
    for (rel, _) in test_files().iter().skip(1) {
        let stem = Path::new(rel).file_stem().unwrap().to_string_lossy();
        assert!(
            root.lines()
                .any(|line| line.trim() == format!("mod {stem};")),
            "{rel} exists but memory/tests.rs does not declare `mod {stem};`"
        );
    }
}

#[test]
fn reference_doc_names_the_new_files() {
    let doc_path = repo_root().join("docs/reference/user-prompt-submit-hook-api.md");
    let doc = fs::read_to_string(&doc_path).expect("reference doc is readable");
    for name in [
        "`memory/text.rs`",
        "`memory/language.rs`",
        "`memory/scoring.rs`",
        "`smart_stop_words.txt`",
        "user_prompt/memory/tests/",
    ] {
        assert!(
            doc.contains(name),
            "docs/reference/user-prompt-submit-hook-api.md must mention {name}"
        );
    }
    assert!(
        !doc.contains("mod tests` of `memory.rs`") && !doc.contains("mod tests of memory.rs"),
        "the doc still says the tests live inside memory.rs"
    );
    for (rel, _) in test_files().iter().skip(1) {
        let file = Path::new(rel).file_name().unwrap().to_string_lossy();
        assert!(
            doc.contains(&format!("`{file}`")),
            "the doc's test list must name `{file}`"
        );
    }
}
