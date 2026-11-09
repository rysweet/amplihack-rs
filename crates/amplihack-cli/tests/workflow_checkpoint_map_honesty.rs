//! Honesty guards for `inventory_map.toml` (#1554, PR #2020, C-q).
//!
//! `validate()` in `workflow_checkpoint_inventory.rs` is the structural gate
//! and is not changed here. These guards are separate tests that close the
//! gaps the consensus round named without relaxing or extending that gate:
//!
//! * `assertions_python` must equal the length of the definition's
//!   `assertions` array in cc's pinned inventory parts, so it cannot be typed
//!   by hand (the 217/27 mistake);
//! * `assertions_rust` may not claim more checks than the target body makes;
//! * a target claimed by more than eight rows needs a note on every row;
//! * `scan()` must index every `corpus_<action>` in the real corpus runner,
//!   so a return to the `corpus_tests!` macro fails here instead of making
//!   every corpus target unresolvable;
//! * every target lives in a file that a Cargo test target compiles, has no
//!   `return;` before its last assertion and reads no `.py` file;
//! * the workflows-crate projection test declares `checkpoint_projection`
//!   and calls its `project()` (the Rust `PhaseProjection`).
//!
//! Checks `validate()` already makes (meta sha, dispositions, pins, missing
//! and invented rows, provenance with a `ref_id`) are not repeated here. The
//! source walker fails closed.
//!
//! None of these reads a `.py` file: reference IDs and counts come only from
//! `inventory_ids.json` and the pinned `inventory.json.part-*`, which stay
//! tracked after C-r. Execution (every target listed by `cargo test --
//! --list` and reported `ok`) is proved off-tree by the ledger, not here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::workflow_checkpoint_inventory::{
    Ids, REMOVAL_CONTROL, ROLE_PENDING_FILE, RustTestIndex, load_ids, scan,
};

const DIR: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint";
const PARTS: [&str; 2] = ["inventory.json.part-1", "inventory.json.part-2"];
const CC_SHA256: &str = "0c3e19b0c72c92a3bd2a140acedcf03f311cb11e9b2c336abf8d3797cc6c788f";
const CORPUS_RS: &str = "crates/amplihack-cli/tests/workflow_checkpoint_corpus.rs";
const PROJECTION_TEST: &str =
    "crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs";
/// A target named by more rows than this needs a note on every such row.
const OVERLOAD_CAP: usize = 8;
const ADJUDICATOR: &str = "workflow-terminal-adjudication::terminal-state-adjudicator";
const COVERAGE_TABLES: [&str; 4] = ["python_test", "procedural", "role", "site"];
/// The only keys a coverage row may hold. A typo such as `assertion_rust`
/// would otherwise be ignored by `validate()` and read as a missing count.
const ROW_KEYS: [&str; 7] = [
    "ref_id",
    "rust_test",
    "disposition",
    // Decision 17.146-2: the tracked .py file still holding the behaviour.
    "pending_file",
    "assertions_python",
    "assertions_rust",
    "note",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn map() -> toml::Value {
    toml::from_str(&read(&format!("{DIR}/inventory_map.toml")))
        .expect("commit a parsable inventory_map.toml")
}

fn ids() -> Ids {
    load_ids(&read(&format!("{DIR}/inventory_ids.json")))
}

fn rows<'a>(map: &'a toml::Value, table: &str) -> &'a [toml::Value] {
    map.get(table)
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn s<'a>(row: &'a toml::Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(toml::Value::as_str)
}

fn has_note(row: &toml::Value) -> bool {
    s(row, "note").is_some_and(|n| !n.trim().is_empty())
}

/// cc's pinned inventory, reassembled from its committed parts. A missing
/// part fails the test; there is no skip.
fn pinned() -> serde_json::Value {
    let mut raw = Vec::new();
    for part in PARTS {
        let path = root().join(DIR).join(part);
        raw.extend(fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    }
    serde_json::from_slice(&raw).expect("pinned inventory parses")
}

/// What the pinned inventory says about one Python test definition.
#[derive(Debug, Clone)]
struct PyDef {
    assertion_lines: Vec<u64>,
    loop_lines: Vec<u64>,
}

fn python_defs(cc: &serde_json::Value) -> BTreeMap<String, PyDef> {
    let lines = |v: &serde_json::Value| -> Vec<u64> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["line"].as_u64())
            .collect()
    };
    cc["python_assets"]
        .as_array()
        .expect("python_assets")
        .iter()
        .flat_map(|a| a["tests"].as_array().expect("tests").iter())
        .map(|t| {
            let mut loop_lines = lines(&t["static_loops"]);
            loop_lines.extend(lines(&t["subtest_sites"]));
            (
                t["id"].as_str().expect("id").to_owned(),
                PyDef {
                    assertion_lines: lines(&t["assertions"]),
                    loop_lines,
                },
            )
        })
        .collect()
}

// ---------------------------------------------------- source inspection

/// Every `.rs` file under `crates/`, `bins/` and `tests/`, keyed by repo path.
/// Fails closed: a root that is missing or holds no `.rs` file, and any
/// directory, entry or file that cannot be read, panics.
fn workspace_sources() -> Vec<(String, String)> {
    walk_sources(&root(), &["crates", "bins", "tests"])
}

fn walk_sources(base: &Path, tops: &[&str]) -> Vec<(String, String)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let entries =
            fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for e in entries {
            let e = e.unwrap_or_else(|err| panic!("entry under {}: {err}", dir.display()));
            let p = e.path();
            let name = e.file_name();
            if name == "target" || name == ".git" || name == "worktrees" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = fs::read_to_string(&p)
                    .unwrap_or_else(|err| panic!("read {}: {err}", p.display()));
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.push((rel, src));
            }
        }
    }
    let mut out = Vec::new();
    for top in tops {
        let dir = base.join(top);
        assert!(dir.is_dir(), "source root {} is missing", dir.display());
        let before = out.len();
        walk(base, &dir, &mut out);
        assert!(out.len() > before, "no .rs files under {}", dir.display());
    }
    out
}

/// Copy of `src` with comments removed and the contents of string, raw
/// string and char literals replaced by spaces. Byte offsets are kept, so a
/// position in the result is the same position in `src`.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = b[i..]
                    .iter()
                    .position(|c| *c == b'\n')
                    .map_or(b.len(), |n| i + n);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let (mut depth, mut j) = (1, i + 2);
                while j < b.len() && depth > 0 {
                    if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                        depth += 1;
                        j += 2;
                    } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'r' if (i == 0 || !ident(b[i - 1]) || b[i - 1] == b'b')
                && matches!(b.get(i + 1), Some(b'"' | b'#')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let body = j + 1;
                let end = b[body..]
                    .windows(close.len())
                    .position(|w| w == close.as_slice())
                    .map_or(b.len(), |n| body + n + close.len());
                blank(&mut out, body, end.saturating_sub(close.len()).max(body));
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i + 1, j.min(b.len()));
                i = j + 1;
            }
            b'\'' => {
                // A char literal ('x', '\n', '\u{7b}') or a lifetime ('a).
                if b.get(i + 1) == Some(&b'\\') {
                    let from = (i + 3).min(b.len());
                    let end = b[from..]
                        .iter()
                        .position(|c| *c == b'\'')
                        .map_or(b.len(), |n| from + n);
                    blank(&mut out, i + 1, end);
                    i = end + 1;
                } else {
                    let ch_len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if b.get(i + 1 + ch_len) == Some(&b'\'') {
                        blank(&mut out, i + 1, i + 1 + ch_len);
                        i += 2 + ch_len;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("blanking keeps UTF-8 boundaries")
}

/// The byte range of the body (between and excluding the outer braces) of
/// `fn name` in `src`. Braces in strings, chars and comments are ignored.
fn fn_body_range(src: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let code = code_only(src);
    let bytes = code.as_bytes();
    let needle = format!("fn {name}");
    let mut from = 0;
    while let Some(at) = code[from..].find(&needle) {
        let start = from + at;
        let after = start + needle.len();
        let boundary_before =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let boundary_after = matches!(bytes.get(after), Some(b'(' | b'<' | b' '));
        if boundary_before && boundary_after {
            let open = after + code[after..].find('{')?;
            let mut depth = 0usize;
            for (k, c) in code[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(open + 1..open + k);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        from = after;
    }
    None
}

/// The body of `fn name` with comments removed and literal contents blanked.
fn fn_body(src: &str, name: &str) -> Option<String> {
    fn_body_range(src, name).map(|r| code_only(src)[r].to_owned())
}

/// The body of `fn name` as written, literals included.
fn fn_body_raw<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    fn_body_range(src, name).map(|r| &src[r])
}

/// Assertion calls in a body already passed through `code_only`: macros and
/// free functions whose name starts with `assert` (`assert!`, `assert_eq!`,
/// `assert_ne!`, `assert_matches!`, `assert_refused(..)`) or `expect_`
/// (`expect_violation(..)`). Method calls such as `.expect(..)` and
/// `.assert_success()` are not counted, so the count can only be low.
fn count_asserts(code: &str) -> usize {
    assert_positions(code).len()
}

fn assert_positions(code: &str) -> Vec<usize> {
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut hits = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if ident(b[i]) && (i == 0 || (!ident(b[i - 1]) && b[i - 1] != b'.')) {
            let start = i;
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            let word = &code[start..i];
            if word.starts_with("assert") || word.starts_with("expect_") {
                let rest = code[i..].trim_start();
                if rest.starts_with("!(") || rest.starts_with('(') {
                    hits.push(start);
                }
            }
        } else {
            i += 1;
        }
    }
    hits
}

/// `<path>::<fn>[#case]` -> (`<path>`, `<fn>`, `case`).
fn split_target(target: &str) -> (&str, &str, Option<&str>) {
    let (key, case) = match target.split_once('#') {
        Some((k, c)) => (k, Some(c)),
        None => (target, None),
    };
    let (path, name) = key.rsplit_once("::").expect("rust_test is <path>::<fn>");
    (path, name, case)
}

/// The corpus case table for `corpus_<action>#<case>`.
fn corpus_case(name: &str, case: &str) -> Option<toml::Value> {
    let action = name.strip_prefix("corpus_")?.replace('_', "-");
    let text = fs::read_to_string(
        root()
            .join(DIR)
            .join("corpus")
            .join(format!("{action}.toml")),
    )
    .ok()?;
    let doc: toml::Value = toml::from_str(&text).ok()?;
    doc.get("case")?
        .as_array()?
        .iter()
        .find(|c| s(c, "name") == Some(case))
        .cloned()
}

/// The checks the corpus runner makes for one case: exit status, stdout and
/// stderr (always compared, empty by default) and one per `expect_files` entry.
fn corpus_checks(case: &toml::Value) -> usize {
    3 + case
        .get("expect_files")
        .and_then(toml::Value::as_table)
        .map_or(0, |t| t.len())
}

/// The assertion count a target can honestly claim.
fn target_checks(target: &str) -> Result<usize, String> {
    let (path, name, case) = split_target(target);
    if let Some(case) = case {
        return corpus_case(name, case)
            .map(|c| corpus_checks(&c))
            .ok_or_else(|| format!("{target}: no such corpus case"));
    }
    let src = fs::read_to_string(root().join(path)).map_err(|e| format!("{target}: {e}"))?;
    fn_body(&src, name)
        .map(|b| count_asserts(&b))
        .ok_or_else(|| format!("{target}: body not found"))
}

fn coverage_targets(map: &toml::Value) -> BTreeMap<String, Vec<String>> {
    let mut by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for table in COVERAGE_TABLES {
        for row in rows(map, table) {
            if let (Some(id), Some(t)) = (s(row, "ref_id"), s(row, "rust_test")) {
                by_target
                    .entry(t.to_owned())
                    .or_default()
                    .push(id.to_owned());
            }
        }
    }
    by_target
}

/// Lexically normalise `a/b/../c` to `a/c`, without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Repo-relative paths of every `.rs` file a Cargo test target compiles as a
/// test root or a `#[path]` module: auto-discovered `tests/*.rs` in crates
/// without `autotests = false`, `[[test]] path`s and every `#[path = ".."]`
/// resolved against the declaring file. `src/` files belong to their crate.
fn compiled_test_files(sources: &[(String, String)]) -> BTreeSet<String> {
    let mut compiled = BTreeSet::new();
    for (rel, src) in sources {
        if rel.contains("/src/") {
            compiled.insert(rel.clone());
        }
        let dir = Path::new(rel).parent().unwrap_or(Path::new(""));
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("#[path = \"")
                && let Some(p) = rest.split('"').next()
            {
                compiled.insert(normalise(&dir.join(p)).to_string_lossy().into_owned());
            }
        }
    }
    for manifest in ["crates", "bins"]
        .iter()
        .flat_map(|top| {
            fs::read_dir(root().join(top))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|e| e.path().join("Cargo.toml"))
        .filter(|p| p.is_file())
    {
        let text = fs::read_to_string(&manifest).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        let crate_dir = manifest
            .parent()
            .unwrap()
            .strip_prefix(root())
            .unwrap()
            .to_path_buf();
        for t in doc
            .get("test")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(p) = s(t, "path") {
                compiled.insert(normalise(&crate_dir.join(p)).to_string_lossy().into_owned());
            }
        }
        let autotests = doc
            .get("package")
            .and_then(|p| p.get("autotests"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if autotests {
            for e in fs::read_dir(root().join(&crate_dir).join("tests"))
                .into_iter()
                .flatten()
                .flatten()
            {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "rs") {
                    compiled.insert(
                        normalise(&crate_dir.join("tests").join(e.file_name()))
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
    }
    compiled
}

// ------------------------------------------- unit tests of the guard tools

#[test]
fn code_only_blanks_strings_chars_and_comments_but_keeps_offsets() {
    let src = "let a = \"{ assert!(x) }\"; // assert!(y) {\nlet c = '{'; /* } assert!(z) */ let r = r#\"}\"#;\n";
    let code = code_only(src);
    assert_eq!(code.len(), src.len());
    assert!(!code.contains("assert"), "{code}");
    assert!(!code.contains('{') && !code.contains('}'), "{code}");
    assert_eq!(code.matches('\n').count(), 2);
}

#[test]
fn code_only_keeps_lifetimes_and_escaped_chars_apart() {
    let code = code_only("fn f<'a>(x: &'a str) -> char { '\\'' }\n");
    assert!(code.contains("fn f<'a>(x: &'a str)"), "{code}");
    assert_eq!(code.matches('{').count(), 1);
    assert_eq!(code.matches('}').count(), 1);
}

#[test]
fn fn_body_finds_the_named_function_only() {
    let src = "#[test]\nfn ab() { assert!(false); }\n#[test]\nfn a() {\n    let s = \"}\";\n    assert_eq!(s, \"}\");\n}\n";
    let body = fn_body(src, "a").expect("body of a");
    assert!(body.contains("assert_eq!"), "{body}");
    assert!(!body.contains("assert!(false)"), "matched fn ab: {body}");
    assert!(fn_body(src, "missing").is_none());
}

#[test]
fn count_asserts_counts_macros_and_assert_helpers_not_methods_or_text() {
    let body = code_only(
        "assert!(a); assert_eq!(b, c); assert_ne!(d, e);\n\
         assert_refused(&out); expect_violation(v, |x| true, \"w\");\n\
         x.expect(\"msg\"); out.assert_success(); // assert!(no)\n\
         let t = \"assert!(no)\"; debug_assert!(z); asserted(q);\n",
    );
    // assert!, assert_eq!, assert_ne!, assert_refused(), expect_violation()
    // and `asserted(` (an `assert*` call). Methods, comments, strings and
    // debug_assert! are not counted.
    assert_eq!(count_asserts(&body), 6);
}

#[test]
fn count_asserts_is_zero_for_a_body_without_checks() {
    assert_eq!(count_asserts(&code_only("let x = 1; run(x);")), 0);
}

#[test]
fn corpus_checks_counts_status_streams_and_each_file_expectation() {
    let case: toml::Value = toml::from_str(
        "name = \"c\"\nexpect_status = 0\n[expect_files]\n\"a\" = \"unchanged\"\n\".git\" = \"present\"\n",
    )
    .unwrap();
    assert_eq!(corpus_checks(&case), 5);
}

#[test]
fn normalise_resolves_parent_components_lexically() {
    assert_eq!(
        normalise(Path::new(
            "crates/amplihack-cli/tests/../../../tests/support/x.rs"
        )),
        PathBuf::from("tests/support/x.rs")
    );
}

#[test]
fn scan_does_not_index_tests_generated_by_a_macro() {
    // The reason the corpus macro must be expanded: scan() reads `fn $fn()`.
    let index = scan(&[(
        "c.rs".into(),
        "macro_rules! m {\n    ($f:ident) => {\n        #[test]\n        fn $f() {}\n    };\n}\nm!(corpus_x);\n".into(),
    )]);
    assert!(!index.contains_key("c.rs::corpus_x"), "{index:?}");
}

#[test]
fn walk_sources_panics_on_a_missing_or_empty_root() {
    let base = root();
    for top in ["target/no-such-dir-for-walk-sources", "docs"] {
        let result = std::panic::catch_unwind(|| walk_sources(&base, &[top]));
        assert!(result.is_err(), "walk_sources returned for {top}");
    }
}

// --------------------------------------------- honesty guards (real map)

#[test]
fn assertions_python_match_the_pinned_inventory_parts() {
    let map = map();
    let defs = python_defs(&pinned());
    assert_eq!(defs.len(), 190, "cc's 190 python_test definitions");
    let mut wrong = Vec::new();
    for row in rows(&map, "python_test") {
        let id = s(row, "ref_id").unwrap_or("");
        let Some(def) = defs.get(id) else {
            wrong.push(format!("{id}: not in the pinned parts"));
            continue;
        };
        let claimed = row
            .get("assertions_python")
            .and_then(toml::Value::as_integer);
        if claimed != Some(def.assertion_lines.len() as i64) {
            wrong.push(format!(
                "{id}: assertions_python {claimed:?}, pinned parts have {}",
                def.assertion_lines.len()
            ));
        }
    }
    assert_eq!(
        rows(&map, "python_test").len(),
        190,
        "one row per python_test"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn assertions_rust_claims_do_not_exceed_the_target_body() {
    let map = map();
    let mut wrong = Vec::new();
    for row in rows(&map, "python_test") {
        let id = s(row, "ref_id").unwrap_or("");
        let Some(target) = s(row, "rust_test") else {
            continue;
        };
        let claimed = row
            .get("assertions_rust")
            .and_then(toml::Value::as_integer)
            .unwrap_or(-1);
        let python = row
            .get("assertions_python")
            .and_then(toml::Value::as_integer)
            .unwrap_or(i64::MAX);
        match target_checks(target) {
            Err(e) => wrong.push(format!("{id}: {e}")),
            Ok(n) if claimed > n as i64 => wrong.push(format!(
                "{id}: assertions_rust {claimed} but {target} makes {n} checks"
            )),
            Ok(_) if claimed < python => wrong.push(format!(
                "{id}: assertions_rust {claimed} < assertions_python {python}"
            )),
            Ok(_) => {}
        }
    }
    assert!(
        !rows(&map, "python_test").is_empty(),
        "[[python_test]] rows"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn targets_claimed_by_more_than_eight_rows_carry_a_note_on_every_row() {
    let map = map();
    let by_target = coverage_targets(&map);
    // 17.146-2: before the first port no row names a target; every row is
    // then pending, never silently empty.
    if by_target.is_empty() {
        let unpended: Vec<String> = COVERAGE_TABLES
            .iter()
            .flat_map(|t| rows(&map, t))
            .filter(|r| s(r, "pending_file").is_none() && s(r, "disposition").is_none())
            .map(|r| s(r, "ref_id").unwrap_or("").to_owned())
            .collect();
        assert!(
            unpended.is_empty(),
            "rows with no target and no pending file: {unpended:?}"
        );
    }
    let mut wrong = Vec::new();
    for table in COVERAGE_TABLES {
        for row in rows(&map, table) {
            let Some(t) = s(row, "rust_test") else {
                continue;
            };
            let claims = by_target[t].len();
            if claims > OVERLOAD_CAP && !has_note(row) {
                wrong.push(format!(
                    "[[{table}]] {}: {t} is claimed by {claims} rows and this row has no note",
                    s(row, "ref_id").unwrap_or("")
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn rows_whose_python_assertions_follow_a_loop_or_subtest_carry_a_note() {
    // cc's inventory does not expand computed loops or subTest sites, so an
    // assertion after one stands for an unknown number of checks. The row
    // says so in its note.
    let map = map();
    let defs = python_defs(&pinned());
    let mut wrong = Vec::new();
    for row in rows(&map, "python_test") {
        // A pending row claims no Rust checks, so it needs no note yet.
        if s(row, "rust_test").is_none() {
            continue;
        }
        let id = s(row, "ref_id").unwrap_or("");
        let Some(def) = defs.get(id) else { continue };
        let Some(first_loop) = def.loop_lines.iter().min() else {
            continue;
        };
        if def.assertion_lines.iter().any(|l| l > first_loop) && !has_note(row) {
            wrong.push(format!(
                "{id}: assertion after the loop at L{first_loop}; add a note"
            ));
        }
    }
    assert!(
        !rows(&map, "python_test").is_empty(),
        "[[python_test]] rows"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn scan_indexes_every_corpus_action_in_the_real_corpus_runner() {
    let index: RustTestIndex = scan(&[(CORPUS_RS.to_owned(), read(CORPUS_RS))]);
    let missing: Vec<String> = amplihack_cli::workflow_checkpoint::ACTIONS
        .iter()
        .map(|a| format!("{CORPUS_RS}::corpus_{}", a.name.replace('-', "_")))
        .filter(|key| !index.contains_key(key))
        .collect();
    assert!(
        missing.is_empty(),
        "scan() cannot see these corpus tests (still macro-generated?): {missing:#?}"
    );
    assert!(
        index.contains_key(&format!("{CORPUS_RS}::corpus_runner_covers_every_action")),
        "corpus_runner_covers_every_action is kept as an explicit #[test]"
    );
}

#[test]
fn corpus_inventory_refs_and_corpus_targets_agree_both_ways() {
    let map = map();
    let ids = ids();
    let known = |id: &str| {
        ids.python_tests.contains(id)
            || ids.procedural.contains(id)
            || ids.roles.contains_key(id)
            || ids.sites.contains(id)
    };
    let mut row_target: BTreeMap<String, String> = BTreeMap::new();
    for table in COVERAGE_TABLES {
        for row in rows(&map, table) {
            if let (Some(id), Some(t)) = (s(row, "ref_id"), s(row, "rust_test")) {
                row_target.insert(id.to_owned(), t.to_owned());
            }
        }
    }
    let mut wrong = Vec::new();
    let mut referenced = 0;
    for e in fs::read_dir(root().join(DIR).join("corpus"))
        .unwrap()
        .flatten()
    {
        let p = e.path();
        let action = p.file_stem().unwrap().to_string_lossy().into_owned();
        let doc: toml::Value = toml::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        for case in doc
            .get("case")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(reference) = s(case, "inventory_ref") else {
                continue;
            };
            referenced += 1;
            let name = s(case, "name").unwrap_or("");
            let want = format!("{CORPUS_RS}::corpus_{}#{name}", action.replace('-', "_"));
            if !known(reference) {
                wrong.push(format!(
                    "{action}#{name}: inventory_ref {reference} is not a reference ID"
                ));
            } else if row_target.get(reference) != Some(&want) {
                wrong.push(format!(
                    "{action}#{name}: the map row for {reference} names {:?}, not {want}",
                    row_target.get(reference)
                ));
            }
        }
    }
    // Both ways: every map row that names a corpus case is matched by a case
    // that names it back (zero of each while those rows are pending).
    let corpus_rows = row_target.values().filter(|t| t.contains("#")).count();
    assert_eq!(
        corpus_rows, referenced,
        "map rows naming a corpus case and corpus cases naming a row differ"
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_unauthorised_41st_role_maps_to_a_test_that_keeps_it_unauthorised() {
    let map = map();
    assert_eq!(ids().roles.get(ADJUDICATOR), Some(&false));
    assert_eq!(
        rows(&map, "role").len(),
        41,
        "41 role rows, the 41st included"
    );
    let row = rows(&map, "role")
        .iter()
        .find(|r| s(r, "ref_id") == Some(ADJUDICATOR))
        .expect("a [[role]] row for the terminal-state-adjudicator");
    if let Some(file) = s(row, "pending_file") {
        // 17.146-1: pending only until the source removal control exists.
        assert_eq!(file, ROLE_PENDING_FILE);
        let index = scan(&workspace_sources());
        assert!(
            !index.contains_key(REMOVAL_CONTROL),
            "the removal control exists, so the 41st role must be mapped"
        );
        return;
    }
    let target = s(row, "rust_test").expect("the 41st role names a rust_test, not a disposition");
    let (path, name, case) = split_target(target);
    assert!(
        case.is_none(),
        "{target}: a named #[test], not a corpus case"
    );
    let src = read(path);
    let body = fn_body(&src, name).unwrap_or_else(|| panic!("{target}: body"));
    let raw = fn_body_raw(&src, name).unwrap();
    assert!(
        raw.contains("terminal-state-adjudicator") && raw.contains("authorised"),
        "{target} must name the role and its authorisation"
    );
    assert!(count_asserts(&body) > 0, "{target} asserts nothing");
}

#[test]
fn provenance_records_the_pinned_parts_sha_and_the_skeleton_command() {
    let map = map();
    let prov = rows(&map, "provenance");
    assert!(!prov.is_empty(), "a [[provenance]] row for the skeleton");
    assert!(
        prov.iter().any(|r| {
            s(r, "sha256") == Some(CC_SHA256)
                && s(r, "description").is_some_and(|d| d.contains("jq") && d.contains("assertions"))
        }),
        "one provenance row records cc's sha256 and the jq command that wrote assertions_python"
    );
}

#[test]
fn coverage_rows_use_only_known_fields_and_counts_only_on_python_tests() {
    let map = map();
    let mut wrong = Vec::new();
    for table in COVERAGE_TABLES {
        for row in rows(&map, table) {
            let id = s(row, "ref_id").unwrap_or("<no ref_id>");
            for key in row.as_table().unwrap().keys() {
                if !ROW_KEYS.contains(&key.as_str()) {
                    wrong.push(format!("[[{table}]] {id}: unknown field {key}"));
                }
            }
            if table != "python_test"
                && (row.get("assertions_python").is_some() || row.get("assertions_rust").is_some())
            {
                wrong.push(format!(
                    "[[{table}]] {id}: assertion counts belong to python_test rows"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn both_destination_read_79_site_tests_are_mapped_as_site_coverage() {
    let map = map();
    let targets: Vec<&str> = rows(&map, "site")
        .iter()
        .filter_map(|r| s(r, "rust_test"))
        .collect();
    let pins: Vec<&str> = rows(&map, "disposition")
        .iter()
        .filter_map(|r| s(r, "pinning_test"))
        .collect();
    for site in [
        "site_1_reports_a_terminal_79_from_the_destination_read_as_79",
        "site_2_reports_a_terminal_79_from_the_destination_read_as_79",
    ] {
        let want = format!("bins/amplihack/tests/workflow_checkpoint_exit79.rs::{site}");
        if targets.is_empty() {
            // 17.146-2: every [[site]] row is still pending with its file.
            // Until a site is ported, each 79 test stays named by its
            // 17.126 disposition, and every site row is pending.
            assert!(
                pins.contains(&want.as_str()),
                "no [[disposition]] pins {want} while the site rows are pending"
            );
            assert!(
                rows(&map, "site")
                    .iter()
                    .all(|r| s(r, "pending_file").is_some()),
                "a [[site]] row with neither a target nor a pending file"
            );
        } else {
            assert!(
                targets.contains(&want.as_str()),
                "no [[site]] row maps to workflow_checkpoint_exit79::{site}"
            );
        }
    }
}

#[test]
fn every_target_is_in_a_file_that_a_cargo_test_target_compiles() {
    let map = map();
    let sources = workspace_sources();
    let compiled = compiled_test_files(&sources);
    let mut wrong = BTreeSet::new();
    for target in coverage_targets(&map).keys() {
        let (path, _, _) = split_target(target);
        if !compiled.contains(path) {
            wrong.insert(format!(
                "{target}: {path} is not compiled by any test target"
            ));
        }
    }
    for row in rows(&map, "disposition") {
        if let Some(pin) = s(row, "pinning_test") {
            let (path, _, _) = split_target(pin);
            if !compiled.contains(path) {
                wrong.insert(format!("{pin}: {path} is not compiled by any test target"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{}",
        wrong.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn no_target_returns_before_its_last_assertion() {
    // scan() only looks at the first two body lines for an env-gated return.
    // A `return;` anywhere before the last check makes a test pass without
    // making every check; a missing tool must fail the test, not skip it.
    let map = map();
    let mut wrong = BTreeSet::new();
    for target in coverage_targets(&map).keys() {
        let (path, name, case) = split_target(target);
        if case.is_some() {
            continue;
        }
        let Ok(src) = fs::read_to_string(root().join(path)) else {
            wrong.insert(format!("{target}: unreadable"));
            continue;
        };
        let Some(body) = fn_body(&src, name) else {
            wrong.insert(format!("{target}: body not found"));
            continue;
        };
        let last = assert_positions(&body)
            .last()
            .copied()
            .unwrap_or(body.len());
        let head = &body[..last];
        // A `return` inside a closure (`|..| { return x; }`) is allowed; a
        // bare statement `return;` is the skip pattern.
        if head.split(';').any(|stmt| {
            let stmt = stmt.trim();
            stmt.ends_with("return") || stmt.ends_with("return Ok(())")
        }) {
            wrong.insert(format!("{target}: returns before its last assertion"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{}",
        wrong.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn no_target_reads_a_python_file() {
    // The map must stay green after C-r deletes the .py files.
    let map = map();
    let mut wrong = BTreeSet::new();
    for target in coverage_targets(&map).keys() {
        let (path, name, case) = split_target(target);
        if case.is_some() {
            continue;
        }
        let src = read(path);
        let Some(raw_body) = fn_body_raw(&src, name) else {
            wrong.insert(format!("{target}: body not found"));
            continue;
        };
        if raw_body.contains(".py\"") || raw_body.contains("python3") {
            wrong.insert(format!("{target}: names a .py file or python3"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{}",
        wrong.into_iter().collect::<Vec<_>>().join("\n")
    );
}

// ------------------------------------------ workflows projection (C-q)

/// Positive: the projection test declares `mod checkpoint_projection` from
/// `tests/integration/checkpoint_projection.rs` and calls its `project()` in
/// code (not in a comment or string).
#[test]
fn the_projection_test_runs_the_rust_phase_projection() {
    const PROJECTION_RS: &str = "tests/integration/checkpoint_projection.rs";
    let src = read(PROJECTION_TEST);
    let code = code_only(&src);
    let dir = Path::new(PROJECTION_TEST).parent().unwrap();
    let mut lines = src.lines().map(str::trim);
    let mut declared = false;
    while let Some(line) = lines.next() {
        if let Some(rel) = line
            .strip_prefix("#[path = \"")
            .and_then(|r| r.strip_suffix("\"]"))
            && normalise(&dir.join(rel)) == Path::new(PROJECTION_RS)
        {
            declared = lines
                .next()
                .is_some_and(|m| m == "mod checkpoint_projection;");
            break;
        }
    }
    assert!(
        declared,
        "{PROJECTION_TEST} declares `mod checkpoint_projection;` from {PROJECTION_RS}"
    );
    assert!(root().join(PROJECTION_RS).is_file(), "{PROJECTION_RS}");
    assert!(
        code.contains("checkpoint_projection::project("),
        "{PROJECTION_TEST} calls checkpoint_projection::project() in code"
    );
}
