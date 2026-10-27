//! Issues #1561 / #1607 — a recipe's own step output must win over a bare
//! upper-case variable inherited from whatever launched the runner.
//!
//! The bundle used to read step outputs as `${UPPER:-${RECIPE_VAR_lower:-}}`,
//! so a stale `QA_EVIDENCE` or `AUTODRIVE_TOOLS_DIR` from a parent run, a
//! shell or a CI job outranked the value the current recipe produced. The fix
//! reads `RECIPE_VAR_<key>` only; nested fields come from the JSON in the base
//! key (`amplifier-bundle/tools/recipe_var.sh`, `worktree_setup_path.sh`).
//!
//! Scope: every recipe directly in `amplifier-bundle/recipes/` and every tool
//! directly in `amplifier-bundle/tools/` for the bare-first and nested-name
//! checks; the declared-key check covers the same recipes.
//!
//! The guards:
//! - no read lets a bare name win over (or fill in for) a `RECIPE_VAR_` read;
//! - no nested `RECIPE_VAR_<key>__<field>` name is read;
//! - every `RECIPE_VAR_` key a recipe step (or a tool it names) reads is
//!   declared in `context:` or produced by an earlier unconditional step;
//! - the bare `NO_MERGE` override can only turn merging off;
//! - the merge round stops, without running any tool, when its tools dir is
//!   empty, relative or not a directory, whatever `AUTODRIVE_TOOLS_DIR` says;
//! - no recipe step or bundle tool reads a step output through a plain bare
//!   name (#1684), beyond a per-read list that can only shrink;
//! - behavioural decoy tests for `rv_field`, `worktree_setup_path.sh` and the
//!   terminal-state probe's inputs.
//!
//! See "Step outputs ignore inherited bare variables" in
//! `docs/reference/auto-drive-to-merge.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use regex::Regex;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/amplihack-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Not recursive: `recipes/tests/` holds harnesses that stand in for the
/// runner and set bare names on purpose.
const RECIPES_DIR: &str = "amplifier-bundle/recipes";

/// Not recursive either: `tools/gh-merge-guard/` is the #1551 shim.
const TOOLS_DIR: &str = "amplifier-bundle/tools";

/// Floors that stop the scan from silently covering nothing.
const MIN_RECIPES: usize = 20;
const MIN_TOOLS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Violation {
    line: usize,
    detector: &'static str,
    bare: String,
    text: String,
}

impl Violation {
    /// Source text only, never an environment value.
    fn render(&self, path: &str) -> String {
        format!("{path}:{}: {}: {}", self.line, self.detector, self.text)
    }
}

/// `RECIPE_VAR_worktree_setup__worktree_path` pairs with
/// `WORKTREE_SETUP_WORKTREE_PATH`: the runner joins a nested field with `__`,
/// the bundle's local copies with `_`.
fn pairs(bare: &str, lower: &str) -> bool {
    let norm = |s: &str| s.to_ascii_uppercase().replace("__", "_");
    norm(bare) == norm(lower) && bare.bytes().all(|b| !b.is_ascii_lowercase())
}

struct Detectors {
    bare_read: Regex,
    recipe_var_read: Regex,
    nested_default: Regex,
    bare_last: Regex,
    bare_in_chain: Regex,
    local_assign: Regex,
    step_start: Regex,
    for_loop: Regex,
    bare_assign: Regex,
    assign: Regex,
}

impl Detectors {
    fn new() -> Self {
        Self {
            bare_read: Regex::new(r"\$\{([A-Z][A-Z0-9_]*)").unwrap(),
            recipe_var_read: Regex::new(r"\$\{?RECIPE_VAR_([A-Za-z0-9_]+)").unwrap(),
            // `${UPPER:-${RECIPE_VAR_…`, `${UPPER:=…`, `${UPPER-…`, quoted or
            // not, whatever the RECIPE_VAR_ name and default.
            nested_default: Regex::new(r#"\$\{([A-Z][A-Z0-9_]*):?[-=]"?\$\{?RECIPE_VAR_"#).unwrap(),
            bare_last: Regex::new(
                r#"\$\{RECIPE_VAR_([A-Za-z0-9_]+):?[-=]"?\$\{?([A-Z][A-Z0-9_]*)\b"#,
            )
            .unwrap(),
            // Any bare name in the default of a RECIPE_VAR_ read, at any depth.
            // `\b` ends the name: `RECIPE_VAR_` before a lower-case key is
            // another recipe value, not a bare name (M4).
            bare_in_chain: Regex::new(
                r#"\$\{RECIPE_VAR_[A-Za-z0-9_]+:?[-=]"?\$\{?([A-Z][A-Z0-9_]*)\b"#,
            )
            .unwrap(),
            // `NAME=…`, `local NAME=…`: a name this script or step sets itself.
            local_assign: Regex::new(
                r"^\s*(?:local\s+|export\s+|readonly\s+|declare\s+(?:-[a-zA-Z]+\s+)?)?([A-Z][A-Z0-9_]*)=",
            )
            .unwrap(),
            // A new recipe step: its locals start empty, since each step is
            // its own process.
            step_start: Regex::new(r#"^\s*-\s+id:\s"#).unwrap(),
            for_loop: Regex::new(r"^\s*for\s+[A-Za-z_][A-Za-z0-9_]*\s+in\s").unwrap(),
            // `X="${UPPER:-}"`, `local X=${UPPER}`, `export X="${UPPER:-d}"`
            bare_assign: Regex::new(
                r#"^\s*(?:local\s+|export\s+|readonly\s+)?([A-Za-z_][A-Za-z0-9_]*)="?\$\{([A-Z][A-Z0-9_]*)(?::?-[^}]*)?\}"?\s*$"#,
            )
            .unwrap(),
            assign: Regex::new(
                r"(?:^|[;&|]\s*|\s)(?:local\s+|export\s+|readonly\s+)?([A-Za-z_][A-Za-z0-9_]*)=",
            )
            .unwrap(),
        }
    }

    /// `[ -n "$X" ] || X="${RECIPE_VAR_x…` or `[ -z "$X" ] && X=…`; returns the
    /// RECIPE_VAR_ name when `line` is such a fallback for local `x`.
    fn fallback_for(&self, line: &str, local: &str) -> Option<String> {
        let x = regex::escape(local);
        let re = Regex::new(&format!(
            r#"(?:\[\[?|test)\s+-(?:n\s+"?\$\{{?{x}\}}?"?\s*\]?\]?\s*\|\||z\s+"?\$\{{?{x}\}}?"?\s*\]?\]?\s*&&)\s*{x}="?\$\{{?RECIPE_VAR_([A-Za-z0-9_]+)"#
        ))
        .unwrap();
        re.captures(line).map(|c| c[1].to_string())
    }

    fn scan(&self, text: &str) -> Vec<Violation> {
        let mut found = Vec::new();
        // local name -> (line, bare name) of its last bare-only assignment
        let mut pending: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
        // Upper-case names this script (or this recipe step) assigns itself.
        let mut locals: BTreeSet<String> = BTreeSet::new();

        for (idx, raw) in text.lines().enumerate() {
            let n = idx + 1;
            let line = raw.trim();
            if line.starts_with('#') {
                continue;
            }
            if self.step_start.is_match(raw) {
                locals.clear();
                pending.clear();
            }
            let mut push = |detector: &'static str, bare: &str| {
                let v = Violation {
                    line: n,
                    detector,
                    bare: bare.to_string(),
                    text: line.to_string(),
                };
                if !found.contains(&v) {
                    found.push(v);
                }
            };

            // nested-default: any bare name whose default is a RECIPE_VAR_ read.
            let mut nested_at = Vec::new();
            for c in self.nested_default.captures_iter(line) {
                let m = c.get(0).unwrap();
                nested_at.push(m.start());
                push("nested-default", &c[1]);
            }

            // bare-last: an empty step output falls through to its own bare name.
            let mut paired_last = Vec::new();
            for c in self.bare_last.captures_iter(line) {
                if pairs(&c[2], &c[1]) {
                    paired_last.push(c[2].to_string());
                    push("bare-last", &c[2]);
                }
            }

            // bare-in-chain: any other inherited bare name after a RECIPE_VAR_
            // read. A name assigned earlier in the same script (or step) is a
            // local, not an inherited read, and may stay as a default.
            for c in self.bare_in_chain.captures_iter(line) {
                let bare = &c[1];
                if !paired_last.iter().any(|b| b == bare) && !locals.contains(bare) {
                    push("bare-in-chain", bare);
                }
            }

            let recipe_vars: Vec<(usize, String)> = self
                .recipe_var_read
                .captures_iter(line)
                .map(|c| (c.get(0).unwrap().start(), c[1].to_string()))
                .collect();
            let bare_reads: Vec<(usize, String)> = self
                .bare_read
                .captures_iter(line)
                .map(|c| (c.get(0).unwrap().start(), c[1].to_string()))
                .filter(|(at, _)| !nested_at.contains(at))
                .collect();

            // candidate-loop: a `for` list that tries the bare name as well as
            // its RECIPE_VAR_ partner, in either order.
            if self.for_loop.is_match(line) {
                for (_, bare) in &bare_reads {
                    if recipe_vars.iter().any(|(_, lower)| pairs(bare, lower)) {
                        push("candidate-loop", bare);
                    }
                }
            }

            // single-line-or: the bare name first, then `||`, then its partner.
            for (at, bare) in &bare_reads {
                let rest = &line[*at..];
                if let Some(or) = rest.find("||") {
                    let after = at + or;
                    if recipe_vars
                        .iter()
                        .any(|(p, lower)| *p > after && pairs(bare, lower))
                    {
                        push("single-line-or", bare);
                    }
                }
            }

            // two-line-fallback: `X="${UPPER:-}"`, then for the same X a
            // `[ -n "$X" ] || X="${RECIPE_VAR_x:-}"`.
            let mut hit = None;
            for (local, (at, bare, text)) in &pending {
                if let Some(lower) = self.fallback_for(line, local)
                    && pairs(bare, &lower)
                {
                    hit = Some((local.clone(), *at, bare.clone(), text.clone()));
                    break;
                }
            }
            if let Some((local, at, bare, text)) = hit {
                pending.remove(&local);
                let v = Violation {
                    line: at,
                    detector: "two-line-fallback",
                    bare,
                    text,
                };
                if !found.contains(&v) {
                    found.push(v);
                }
                continue;
            }
            if let Some(c) = self.local_assign.captures(line) {
                locals.insert(c[1].to_string());
            }
            if let Some(c) = self.bare_assign.captures(line) {
                pending.insert(c[1].to_string(), (n, c[2].to_string(), line.to_string()));
                continue;
            }
            // Any other assignment to a tracked local ends its tracking.
            for c in self.assign.captures_iter(line) {
                pending.remove(&c[1]);
            }
        }
        found.sort_by_key(|v| v.line);
        found
    }
}

fn scan(text: &str) -> Vec<Violation> {
    Detectors::new().scan(text)
}

/// `(repo-relative path, text)` of every `*.<ext>` directly in `dir`, sorted.
/// Fails, rather than returning too little, below `min` files.
fn files_in(dir: &str, ext: &str, min: usize) -> Vec<(String, String)> {
    let root = repo_root();
    let abs = root.join(dir);
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&abs)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", abs.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == ext))
        .collect();
    entries.sort();
    assert!(
        entries.len() >= min,
        "found only {} *.{ext} files under {dir} (want at least {min}); the guard \
         would pass without inspecting the bundle",
        entries.len()
    );
    entries
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(&root).unwrap().display().to_string();
            let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {rel}: {e}"));
            (rel, text)
        })
        .collect()
}

fn recipe_files() -> Vec<(String, String)> {
    files_in(RECIPES_DIR, "yaml", MIN_RECIPES)
}

fn tool_files() -> Vec<(String, String)> {
    files_in(TOOLS_DIR, "sh", MIN_TOOLS)
}

/// Every file the guard reads: the recipes, then the tools.
fn scanned_files() -> Vec<(String, String)> {
    let mut files = recipe_files();
    files.extend(tool_files());
    files
}

const NESTED_NAME: &str = r"RECIPE_VAR_[A-Za-z0-9]+(_[A-Za-z0-9]+)*__[A-Za-z0-9_]+";

/// The code part of one shell line: the text before a `#` that starts a
/// comment. Bash starts a comment only at the start of a word, so a `#` inside
/// single or double quotes, after a backslash, or inside a word (`${#x}`,
/// `$#`, `a#b`) is code. A quote left open at the end of the line keeps the
/// rest of the line as code, which can only make the scan see more.
fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let (mut sq, mut dq, mut esc) = (false, false, false);
    // Index of the last escaped byte: `a\ #b` is one word, not a comment.
    let mut escaped_at = usize::MAX;
    for (i, &c) in b.iter().enumerate() {
        if esc {
            esc = false;
            escaped_at = i;
            continue;
        }
        match c {
            b'\\' if !sq => esc = true,
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b'#' if !sq && !dq => {
                let word_start = i == 0
                    || (escaped_at != i - 1
                        && matches!(b[i - 1], b' ' | b'\t' | b';' | b'&' | b'|' | b'(' | b')'));
                if word_start {
                    return &line[..i];
                }
            }
            _ => {}
        }
    }
    line
}

/// `(line, name)` of every nested name in the code part of `text`, in any
/// form: `${RECIPE_VAR_a__b:-}`, `$RECIPE_VAR_a__b`, `printenv RECIPE_VAR_a__b`
/// or `"RECIPE_VAR_a__b"`. The match is not anchored on `$` (G1).
fn nested_reads(text: &str) -> Vec<(usize, String)> {
    let re = Regex::new(NESTED_NAME).unwrap();
    let mut found = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        for m in re.find_iter(strip_comment(raw)) {
            found.push((i + 1, m.as_str().to_string()));
        }
    }
    found
}

/// A recipe step as the declared-key check sees it.
struct Step {
    id: String,
    /// The bash body, plus the text of every bundle tool the body names.
    reads: BTreeSet<String>,
    output: Option<String>,
    conditional: bool,
}

/// Top-level keys a RECIPE_VAR_ name refers to: `pr_publish_result__pr_url`
/// is the `pr_publish_result` key.
///
/// Stripping a name to its base key is sound because no bundle step reads a
/// nested name: the `nested-read` detector fails on any `RECIPE_VAR_<a>__<b>`.
/// Every field is taken from the JSON in the base `RECIPE_VAR_<a>`, which the
/// runner writes for each declared key, so declaring `<a>` in `context:` is
/// what stops a parent run's value from reaching the step.
fn base_key(k: &str) -> &str {
    k.split("__").next().unwrap_or(k)
}

fn recipe_var_keys(text: &str) -> BTreeSet<String> {
    let re = Regex::new(r"\$\{?RECIPE_VAR_([A-Za-z0-9_]+)").unwrap();
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(|l| {
            re.captures_iter(l)
                .map(|c| c[1].to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `(declared context keys, steps)` of a recipe. `tools` is
/// `(file name, text)` of the bundle tools: a step whose body names one reads
/// what that tool reads.
fn parse_recipe(yaml: &str, tools: &[(String, String)]) -> (BTreeSet<String>, Vec<Step>) {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("recipe parses as YAML");
    let context: BTreeSet<String> = doc
        .get("context")
        .and_then(|c| c.as_mapping())
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut steps = Vec::new();
    for s in doc
        .get("steps")
        .and_then(|s| s.as_sequence())
        .into_iter()
        .flatten()
    {
        let id = s
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let body = s.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let mut reads = recipe_var_keys(body);
        for (name, text) in tools {
            if body.contains(name.as_str()) {
                reads.extend(recipe_var_keys(text));
            }
        }
        steps.push(Step {
            id,
            reads,
            output: s.get("output").and_then(|v| v.as_str()).map(str::to_string),
            conditional: s.get("condition").is_some(),
        });
    }
    (context, steps)
}

/// Every RECIPE_VAR_ read the runner would not replace, as
/// `(step id, key, kind)`. A key is safe when it is declared in `context:`,
/// or produced by an earlier step that has no `condition:`. Otherwise the
/// step sees a parent run's inherited `RECIPE_VAR_<key>`:
/// `undeclared` (nothing declares or produces it), `read-before-producer`, or
/// `read-after-conditional-producer`.
fn undeclared_reads(yaml: &str, tools: &[(String, String)]) -> Vec<(String, String, &'static str)> {
    let (context, steps) = parse_recipe(yaml, tools);
    let mut bad = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        for k in &step.reads {
            let base = base_key(k);
            if context.contains(base) {
                continue;
            }
            let producers: Vec<(usize, &Step)> = steps
                .iter()
                .enumerate()
                .filter(|(_, s)| s.output.as_deref() == Some(base))
                .collect();
            let kind = if producers.is_empty() {
                "undeclared"
            } else if producers.iter().any(|(j, s)| *j < i && !s.conditional) {
                continue;
            } else if producers.iter().all(|(j, _)| *j >= i) {
                "read-before-producer"
            } else {
                "read-after-conditional-producer"
            };
            bad.push((step.id.clone(), k.clone(), kind));
        }
    }
    bad
}

fn step_body(yaml: &str, id: &str) -> Option<String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    doc.get("steps")?
        .as_sequence()?
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(id))?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

fn read_repo_file(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn path_env() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
}

fn jq_on_path() -> bool {
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skip: `jq` not on PATH");
    }
    ok
}

/// Run `script` with bash under an environment holding only PATH and `env`.
fn bash_clean(script: &str, env: &[(&str, &str)], cwd: &Path) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .env_clear()
        .env("PATH", path_env())
        .current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn bash")
}

// ── Bare names never win ─────────────────────────────────────────────────────

#[test]
fn no_bundle_read_lets_a_bare_name_win_over_recipe_var() {
    let mut report = Vec::new();
    for (path, text) in scanned_files() {
        for v in scan(&text) {
            report.push(v.render(&path));
        }
    }
    assert!(
        report.is_empty(),
        "{} step-output reads let an inherited bare variable win over the recipe's \
         own RECIPE_VAR_ value (#1561, #1607):\n{}\n\n\
         Read the step output as \"${{RECIPE_VAR_<lower>:-<default>}}\" and drop the \
         bare name, in either order.",
        report.len(),
        report.join("\n")
    );
}

#[test]
fn the_detectors_flag_every_spelling_of_the_bug() {
    for (text, detector) in [
        (r#"X="${FOO:-${RECIPE_VAR_foo:-}}""#, "nested-default"),
        (r#"X="${FOO_BAR:-${RECIPE_VAR_x:-}}""#, "nested-default"),
        (r#"X=${FOO:-$RECIPE_VAR_foo}"#, "nested-default"),
        (r#"X="${FOO-${RECIPE_VAR_foo:-}}""#, "nested-default"),
        (
            r#": "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-}}""#,
            "nested-default",
        ),
        (
            r#"for C in "${PR_NUMBER:-}" "${RECIPE_VAR_pr_number:-}"; do"#,
            "candidate-loop",
        ),
        (
            r#"for C in "${RECIPE_VAR_pr_number:-}" "${PR_NUMBER:-}"; do"#,
            "candidate-loop",
        ),
        (
            "X=\"${FOO:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"",
            "two-line-fallback",
        ),
        (
            "X=\"${FOO:-}\"\n[ -z \"$X\" ] && X=\"${RECIPE_VAR_foo:-}\"",
            "two-line-fallback",
        ),
        (
            r#"X="${FOO:-}" || X="${RECIPE_VAR_foo:-}""#,
            "single-line-or",
        ),
        (r#"X="${RECIPE_VAR_foo:-${FOO:-}}""#, "bare-last"),
        (r#"X="${RECIPE_VAR_foo:-$FOO}""#, "bare-last"),
        (
            r#"X="${RECIPE_VAR_foo:-${FOO_DEFAULT:-}}""#,
            "bare-in-chain",
        ),
        (
            r#"W="${RECIPE_VAR_worktree_setup:-${REPO_PATH:-}}""#,
            "bare-in-chain",
        ),
        // A local of another recipe step is an inherited name in this one.
        (
            "  - id: \"a\"\n    FOO=1\n  - id: \"b\"\n    X=\"${RECIPE_VAR_x:-${FOO:-}}\"",
            "bare-in-chain",
        ),
    ] {
        let found = scan(text);
        assert!(
            found.iter().any(|v| v.detector == detector),
            "`{text}` must be flagged by {detector}; got {found:?}"
        );
    }
}

#[test]
fn the_detectors_accept_the_fixed_and_unrelated_spellings() {
    for text in [
        r#"X="${RECIPE_VAR_foo:-}""#,
        r#"MODE="${RECIPE_VAR_terminal_gate_mode:-probe}""#,
        r#"X="${RECIPE_VAR_x:-${RECIPE_VAR_y:-}}""#,
        r#": "${TMPDIR:=/tmp}""#,
        r#"REPO="${REPO_PATH:-}""#,
        "local FOO_DEFAULT=1\nX=\"${RECIPE_VAR_foo:-${FOO_DEFAULT:-}}\"",
        r#"for D in "${AMPLIHACK_HOME:-}" "${HOME:-}"; do"#,
        r#"[ -n "${CI:-}" ] || echo "${RECIPE_VAR_ci_evidence:-}""#,
        "# was ${FOO:-${RECIPE_VAR_foo:-}}",
        "X=\"${FOO:-}\"\necho \"$X\"",
        "X=\"${FOO:-}\"\nX=\"$(cat f)\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"",
        r#"[ "$(_rv_truthy "${NO_MERGE:-}")" = "true" ]"#,
    ] {
        let found = scan(text);
        assert!(
            found.is_empty(),
            "`{text}` must not be flagged; got {found:?}"
        );
    }
}

// ── Nested names are never read ──────────────────────────────────────────────

#[test]
fn no_bundle_file_reads_a_nested_recipe_var_name() {
    let mut report = Vec::new();
    for (path, text) in scanned_files() {
        for (line, name) in nested_reads(&text) {
            let src = text.lines().nth(line - 1).unwrap_or("").trim();
            report.push(format!("{path}:{line}: {src}  [{name}]"));
        }
    }
    assert!(
        report.is_empty(),
        "{} reads of a nested name RECIPE_VAR_<key>__<field> (#1561). The runner writes \
         a nested name only for fields the current object has, so a missing field keeps \
         a parent run's value. Read the base key (`rv_field <key> <field>` from \
         recipe_var.sh) and declare <key> in context:\n{}",
        report.len(),
        report.join("\n")
    );
}

#[test]
fn the_nested_read_detector_flags_reads_and_skips_comments() {
    for src in [
        r#"RUN_DIR="${RECIPE_VAR_worktree_setup__worktree_path:-}""#,
        r#"RUN_DIR="$RECIPE_VAR_worktree_setup__worktree_path""#,
        "RUN_DIR=$(printenv RECIPE_VAR_worktree_setup__worktree_path)",
        r#"echo "a # b" ${RECIPE_VAR_a__b:-}"#,
    ] {
        assert_eq!(nested_reads(src).len(), 1, "not flagged: {src}");
    }
    for src in [
        "# was: ${RECIPE_VAR_worktree_setup__worktree_path:-}",
        "x=1  # RECIPE_VAR_a__b in a trailing comment",
        r#"[[ $n == "RECIPE_VAR_${key}__"* ]]"#,
        r#"rv_field worktree_setup worktree_path"#,
    ] {
        assert!(nested_reads(src).is_empty(), "flagged: {src}");
    }
}

// ── Every key read is declared ───────────────────────────────────────────────

#[test]
fn every_recipe_var_a_recipe_reads_is_declared() {
    let tools: Vec<(String, String)> = tool_files()
        .into_iter()
        .map(|(p, t)| (p.rsplit('/').next().unwrap().to_string(), t))
        .collect();
    let mut bad = Vec::new();
    for (path, yaml) in recipe_files() {
        for (id, key, kind) in undeclared_reads(&yaml, &tools) {
            bad.push(format!("{path}: {id}: RECIPE_VAR_{key}: {kind}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} RECIPE_VAR_ reads the runner would not replace for this run (#1561): \
         declare each key in the recipe's `context:` with a \"\" default:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_declared_key_check_flags_each_unsafe_read() {
    let tools = vec![(
        "reads_x.sh".to_string(),
        "echo \"${RECIPE_VAR_tool_key:-}\"\n".to_string(),
    )];
    let yaml = r#"
context:
  declared: ""
steps:
  - id: "early"
    type: "bash"
    command: |
      echo "${RECIPE_VAR_declared:-}" "${RECIPE_VAR_later:-}" "${RECIPE_VAR_nowhere:-}"
  - id: "maybe"
    type: "bash"
    condition: "x == 'y'"
    command: "echo hi"
    output: "maybe_out"
  - id: "produce"
    type: "bash"
    command: "echo hi"
    output: "later"
  - id: "late"
    type: "bash"
    command: |
      echo "${RECIPE_VAR_later:-}" "${RECIPE_VAR_maybe_out:-}"
      bash reads_x.sh
"#;
    let got: BTreeSet<(String, String, &str)> =
        undeclared_reads(yaml, &tools).into_iter().collect();
    let want: BTreeSet<(String, String, &str)> = [
        ("early", "later", "read-before-producer"),
        ("early", "nowhere", "undeclared"),
        ("late", "maybe_out", "read-after-conditional-producer"),
        ("late", "tool_key", "undeclared"),
    ]
    .into_iter()
    .map(|(a, b, c)| (a.to_string(), b.to_string(), c))
    .collect();
    assert_eq!(got, want);
}

// ── NO_MERGE can only turn merging off ───────────────────────────────────────

/// `no_merge_flag` from recipe_var.sh for one (recipe value, bare value) pair;
/// `None` leaves the variable unset.
fn no_merge_flag(recipe: Option<&str>, bare: Option<&str>) -> (String, String) {
    let tool = repo_root().join(TOOLS_DIR).join("recipe_var.sh");
    let mut env = Vec::new();
    if let Some(v) = recipe {
        env.push(("RECIPE_VAR_no_merge", v));
    }
    if let Some(v) = bare {
        env.push(("NO_MERGE", v));
    }
    let script = format!(". '{}' && no_merge_flag test/step", tool.display());
    let out = bash_clean(&script, &env, &repo_root());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn the_no_merge_override_can_only_turn_merging_off() {
    let values = [
        None,
        Some(""),
        Some("true"),
        Some(" YES "),
        Some("On"),
        Some("1"),
        Some("false"),
        Some("0"),
        Some("maybe"),
    ];
    let truthy = |v: Option<&str>| {
        matches!(
            v.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
            Some("true" | "1" | "yes" | "on")
        )
    };
    for recipe in values {
        for bare in values {
            let (got, err) = no_merge_flag(recipe, bare);
            let want = truthy(recipe) || truthy(bare);
            assert_eq!(
                got,
                want.to_string(),
                "recipe no_merge={recipe:?}, bare NO_MERGE={bare:?}"
            );
            let overrode = !truthy(recipe) && truthy(bare);
            assert_eq!(
                err.contains("from bare NO_MERGE env (override, #1561)"),
                overrode,
                "the INFO line appears exactly when the bare name changes the answer \
                 (recipe={recipe:?}, bare={bare:?}): {err}"
            );
        }
    }
}

#[test]
fn only_recipe_var_sh_reads_the_bare_no_merge() {
    let re = Regex::new(r"\$\{?NO_MERGE\b").unwrap();
    let mut readers = BTreeSet::new();
    for (path, text) in scanned_files() {
        if text.lines().any(|l| re.is_match(strip_comment(l))) {
            readers.insert(path);
        }
    }
    let want: BTreeSet<String> = [format!("{TOOLS_DIR}/recipe_var.sh")].into_iter().collect();
    assert_eq!(
        readers, want,
        "a bare NO_MERGE is read only through no_merge_flag in recipe_var.sh, so the \
         override cannot be applied in a way that turns merging on"
    );
    for (recipe, step) in [
        ("autodrive-build.yaml", "step-01b-no-merge-directive"),
        ("autodrive-build.yaml", "step-03-resolve-pr"),
        ("workflow-terminal-state.yaml", "probe-terminal-state"),
    ] {
        let body = step_body(&read_repo_file(&format!("{RECIPES_DIR}/{recipe}")), step)
            .unwrap_or_else(|| panic!("{recipe} has no step {step}"));
        assert!(
            body.contains("no_merge_flag "),
            "{recipe} {step} decides no_merge without no_merge_flag"
        );
    }
}

// ── The merge round's tools dir fails closed ─────────────────────────────────

/// The merge-round steps that run a tool from `autodrive_tools_dir`.
fn merge_round_tool_steps() -> Vec<(String, String)> {
    let yaml = read_repo_file(&format!("{RECIPES_DIR}/autodrive-merge-round.yaml"));
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|s| {
            let body = s.get("command")?.as_str()?;
            body.contains("RECIPE_VAR_autodrive_tools_dir")
                .then(|| (s["id"].as_str().unwrap().to_string(), body.to_string()))
        })
        .collect()
}

/// A tools dir whose every tool writes `marker` when run or sourced.
fn decoy_tools(dir: &Path, marker: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for name in [
        "autodrive_merge_ready_files.sh",
        "autodrive_platform_facts.sh",
        "autodrive_round_evidence.sh",
        "autodrive_trust.sh",
    ] {
        std::fs::write(
            dir.join(name),
            format!("echo {name} >> '{}'\n", marker.display()),
        )
        .unwrap();
    }
}

#[test]
fn the_merge_round_runs_no_tool_from_an_unusable_tools_dir() {
    let steps = merge_round_tool_steps();
    assert!(
        steps.len() >= 6,
        "only {} merge-round steps read autodrive_tools_dir",
        steps.len()
    );
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let marker = tmp.path().join("marker");
    let decoy = tmp.path().join("decoy-tools");
    decoy_tools(&decoy, &marker);
    decoy_tools(&repo.join("rel-tools"), &marker);
    let plain = tmp.path().join("plain-file");
    std::fs::write(&plain, "x").unwrap();
    let decoy_s = decoy.display().to_string();
    let plain_s = plain.display().to_string();
    let repo_s = repo.display().to_string();
    let home = tmp.path().display().to_string();
    for (id, body) in &steps {
        // Positive control: the same step does run a tool from a usable dir,
        // so a step that stops early for another reason cannot pass below.
        let _ = std::fs::remove_file(&marker);
        bash_clean(
            body,
            &[
                ("REPO_PATH", &repo_s),
                ("HOME", &home),
                ("RECIPE_VAR_autodrive_tools_dir", &decoy_s),
            ],
            &repo,
        );
        assert!(
            marker.exists(),
            "{id}: control: no tool ran from a valid autodrive_tools_dir"
        );
        for (what, value) in [
            ("unset", None),
            ("empty", Some("")),
            ("relative", Some("rel-tools")),
            ("not a directory", Some(plain_s.as_str())),
            ("two lines", Some("/tmp\n/x")),
        ] {
            let _ = std::fs::remove_file(&marker);
            let mut env = vec![
                ("REPO_PATH", repo_s.as_str()),
                ("HOME", home.as_str()),
                ("AUTODRIVE_TOOLS_DIR", decoy_s.as_str()), // deliberately stale (#1607)
            ];
            if let Some(v) = value {
                env.push(("RECIPE_VAR_autodrive_tools_dir", v));
            }
            let out = bash_clean(body, &env, &repo);
            assert!(
                !out.status.success() && !marker.exists(),
                "{id}: autodrive_tools_dir {what}: want a non-zero exit and no tool run; \
                 rc={:?} ran={} stderr={}",
                out.status.code(),
                marker.exists(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

// ── Behavioural decoys ───────────────────────────────────────────────────────

/// `rv_field key field` from recipe_var.sh under `env` only.
fn rv_field(key: &str, field: &str, env: &[(&str, &str)]) -> String {
    let tool = repo_root().join(TOOLS_DIR).join("recipe_var.sh");
    let script = format!(
        ". '{}' && v=\"$(rv_field '{key}' '{field}')\" && printf '%s' \"$v\"",
        tool.display()
    );
    let out = bash_clean(&script, env, &repo_root());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn rv_field_reads_the_base_key_and_ignores_stale_names() {
    if !jq_on_path() {
        return;
    }
    let own = r#"{"terminal_state":"FAILED_MEANINGFUL_DIFF","terminal_success":false,"commits_ahead":0,"none":null}"#;
    let stale = [
        (
            "RECIPE_VAR_terminal_state__terminal_state",
            "IMPLEMENTED_VERIFIED",
        ), // deliberately stale (#1561)
        ("RECIPE_VAR_terminal_state__terminal_reason", "stale-reason"), // deliberately stale (#1561)
        ("TERMINAL_STATE_TERMINAL_STATE", "IMPLEMENTED_VERIFIED"), // deliberately stale (#1561)
    ];
    let with_own: Vec<(&str, &str)> = std::iter::once(("RECIPE_VAR_terminal_state", own))
        .chain(stale)
        .collect();
    assert_eq!(
        rv_field("terminal_state", "terminal_state", &with_own),
        "FAILED_MEANINGFUL_DIFF"
    );
    assert_eq!(
        rv_field("terminal_state", "terminal_success", &with_own),
        "false"
    );
    assert_eq!(rv_field("terminal_state", "commits_ahead", &with_own), "0");
    assert_eq!(
        rv_field("terminal_state", "terminal_reason", &with_own),
        "",
        "a missing field never falls back"
    );
    assert_eq!(rv_field("terminal_state", "none", &with_own), "");
    assert_eq!(
        rv_field("terminal_state", "terminal_state", &stale),
        "",
        "an unset base key never falls back"
    );
    assert_eq!(
        rv_field("terminal_state__terminal", "state", &stale),
        "",
        "a nested key is refused"
    );
    for bad in ["not json", "[1]", "{\"a\":1} {\"b\":2}", "''"] {
        let env = [("RECIPE_VAR_terminal_state", bad)];
        assert_eq!(rv_field("terminal_state", "a", &env), "", "value {bad:?}");
    }
}

/// `git init` with one commit (plumbing, no hooks) at `dir`.
fn git_repo(dir: &Path, home: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let out = amplihack_git::command_in(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    let tree = git(&["write-tree"]);
    let commit = git(&["commit-tree", &tree, "-m", "init"]);
    git(&["update-ref", "HEAD", &commit]);
}

/// `worktree_setup_path.sh test-step` with `env`: (stdout, exit code).
fn worktree_setup_path(env: &[(&str, &str)], home: &Path) -> (String, i32) {
    let tool = repo_root().join(TOOLS_DIR).join("worktree_setup_path.sh");
    let home_s = home.display().to_string();
    let mut all = vec![("HOME", home_s.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    all.extend_from_slice(env);
    let out = bash_clean(&format!("bash '{}' test-step", tool.display()), &all, home);
    (
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn worktree_setup_path_accepts_only_this_runs_resolved_worktree_root() {
    if !jq_on_path() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let own = base.join("own");
    let stale = base.join("stale");
    git_repo(&own, &base);
    git_repo(&stale, &base);
    std::fs::create_dir_all(own.join("sub")).unwrap();
    std::os::unix::fs::symlink(&own, base.join("link")).unwrap();
    let ws = |p: &str| serde_json::json!({ "worktree_path": p, "branch_name": "b" }).to_string();
    let own_s = own.display().to_string();
    let stale_s = stale.display().to_string();
    let decoys = [
        ("WORKTREE_SETUP_WORKTREE_PATH", stale_s.as_str()), // deliberately stale (#1561)
        ("RECIPE_VAR_worktree_setup__worktree_path", stale_s.as_str()), // deliberately stale (#1561)
    ];
    let run = |value: Option<String>| {
        let mut env: Vec<(&str, &str)> = decoys.to_vec();
        let v;
        if let Some(x) = value {
            v = x;
            env.push(("RECIPE_VAR_worktree_setup", v.as_str()));
        }
        worktree_setup_path(&env, &base)
    };
    assert_eq!(
        run(Some(ws(&own_s))),
        (own_s.clone(), 0),
        "own resolved worktree root"
    );
    assert_eq!(
        run(None).1,
        3,
        "unset base key never falls back to stale names"
    );
    assert_eq!(run(Some(String::new())).1, 3);
    assert_eq!(run(Some("{\"branch_name\":\"b\"}".into())).1, 3);
    assert_eq!(run(Some("not json".into())).1, 4);
    assert_eq!(run(Some("[1]".into())).1, 4);
    for (what, p) in [
        ("a subdirectory", own.join("sub").display().to_string()),
        ("a symlink", base.join("link").display().to_string()),
        ("relative", "own".to_string()),
        ("two lines", format!("{own_s}\n{stale_s}")),
        ("missing", base.join("gone").display().to_string()),
        ("unresolved", format!("{}/../own", own_s)),
    ] {
        assert_eq!(
            run(Some(ws(&p))),
            (String::new(), 5),
            "{what} worktree_path is rejected"
        );
    }
}

#[test]
fn the_terminal_probe_uses_recipe_values_over_bare_decoys() {
    if !jq_on_path() {
        return;
    }
    let body = step_body(
        &read_repo_file(&format!("{RECIPES_DIR}/workflow-terminal-state.yaml")),
        "probe-terminal-state",
    )
    .expect("probe-terminal-state");
    let lines: Vec<&str> = body.lines().collect();
    let pos = |pred: &dyn Fn(&str) -> bool, what: &str| {
        lines
            .iter()
            .position(|l| pred(l.trim_start()))
            .unwrap_or_else(|| panic!("probe has no {what}"))
    };
    let start = pos(&|l| l.starts_with("RV_TOOLS=\"\""), "tools lookup");
    let end = pos(
        &|l| l.starts_with("[ -n \"$TARGET_REPO_DIR\" ]"),
        "TARGET_REPO_DIR check",
    );
    // The runner renders a step's {{key}} as "$RECIPE_VAR_key".
    let rendered = Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}")
        .unwrap()
        .replace_all(&lines[start..end].join("\n"), "\"$$RECIPE_VAR_$1\"")
        .to_string();
    let script = format!(
        "set -euo pipefail\nfail_terminal_state() {{ echo \"FAIL $*\"; exit 9; }}\n{rendered}\n\
         printf '%s|%s|%s|%s|%s|%s|%s|%s\\n' \"$TARGET_REPO_DIR\" \"$BRANCH_INPUT\" \"$BASE_INPUT\" \
         \"$PR_NUMBER_INPUT\" \"$PR_URL_INPUT\" \"$GOAL_ALREADY_MET_INPUT\" \"$NO_MERGE_DIRECTIVE\" \
         \"$(active_should_merge)\"\n"
    );
    let root = repo_root().display().to_string();
    let out = bash_clean(
        &script,
        &[
            ("AMPLIHACK_HOME", &root),
            ("RECIPE_VAR_repo_path", "/recipe/repo"),
            ("RECIPE_VAR_branch_name", "feat/recipe-1561"),
            ("RECIPE_VAR_base_ref", "recipe-base"),
            ("RECIPE_VAR_pr_number", "4242"),
            ("RECIPE_VAR_pr_url", "https://github.com/o/r/pull/4242"),
            ("RECIPE_VAR_goal_already_met", "true"),
            ("RECIPE_VAR_no_merge", "true"),
            ("REPO_PATH", "/stale/repo"), // deliberately stale (#1561)
            ("WORKTREE_SETUP_WORKTREE_PATH", &root), // deliberately stale (#1561)
            ("RECIPE_VAR_worktree_setup__worktree_path", &root), // deliberately stale (#1561)
            ("BRANCH_NAME", "stale-branch"), // deliberately stale (#1561)
            ("BASE_REF", "stale-base"),   // deliberately stale (#1561)
            ("PR_NUMBER", "1"),           // deliberately stale (#1561)
            ("PR_URL", "https://github.com/stale/stale/pull/1"), // deliberately stale (#1561)
            ("GOAL_ALREADY_MET", "false"), // deliberately stale (#1561)
            ("NO_MERGE", "false"),        // deliberately stale (#1561)
        ],
        &repo_root(),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "probe input block failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "/recipe/repo|feat/recipe-1561|recipe-base|4242|https://github.com/o/r/pull/4242|true|true|false",
        "the recipe's own values reach the probe, and a bare NO_MERGE=false does not \
         clear a recipe no_merge=true"
    );
}

// ── Plain bare reads of step outputs (#1684) ─────────────────────────────────

/// Every plain bare read of a step output that is not converted yet, as
/// `(file, step id, bare name, number of reads)`. A tool script has no steps,
/// so its step id is `""`. Every entry is tracked in #1689. The guard compares
/// the actual reads with this list exactly: a new read (or one more read of a
/// listed name) fails, and so does an entry whose count has dropped, so the
/// list only shrinks. All of these names are scalars, which the runner exports
/// as bare aliases; the object outputs (`local_testing_gate`,
/// `precommit_results`) are never exported that way, and none is listed.
const PLAIN_READS_NOT_YET_CONVERTED: &[(&str, &str, &str, usize)] = &[
    (
        "consensus-publish.yaml",
        "step10-create-pr",
        "ISSUE_NUMBER",
        1,
    ),
    ("consensus-publish.yaml", "step9-commit", "ISSUE_NUMBER", 1),
    (
        "investigation-prep.yaml",
        "normalize-codebase-path",
        "CODEBASE_PATH",
        2,
    ),
    (
        "investigation-prep.yaml",
        "normalize-question",
        "INVESTIGATION_QUESTION",
        2,
    ),
    (
        "smart-validate-summarize.yaml",
        "complete-session",
        "SESSION_INFO",
        1,
    ),
    (
        "workflow-design.yaml",
        "step-06d-goal-already-met-probe",
        "ISSUE_NUMBER",
        1,
    ),
    (
        "workflow-finalize.yaml",
        "step-21-pr-ready",
        "REMOTE_HOST_TYPE",
        1,
    ),
    (
        "workflow-finalize.yaml",
        "step-22b-final-status",
        "REMOTE_HOST_TYPE",
        1,
    ),
    (
        "workflow-publish.yaml",
        "step-15-commit-push",
        "ISSUE_NUMBER",
        1,
    ),
    (
        "workflow-publish.yaml",
        "step-15-commit-push",
        "REMOTE_HOST_TYPE",
        1,
    ),
    (
        "workflow-worktree.yaml",
        "step-04-setup-worktree",
        "ISSUE_NUMBER",
        2,
    ),
    ("git-identity.sh", "", "REMOTE_HOST_TYPE", 1),
    ("workflow_final_status.sh", "", "REMOTE_HOST_TYPE", 2),
    ("workflow_publish_pr.sh", "", "ISSUE_NUMBER", 1),
    ("workflow_publish_pr.sh", "", "REMOTE_HOST_TYPE", 1),
];

/// The caller's own input. `quality-loop.yaml` re-emits it as a step output,
/// but every run sets it, so its bare alias is this run's value.
const RUN_INPUTS: &[&str] = &["task_description"];

/// Every `output:` key of every bundle recipe step, except `RUN_INPUTS`.
fn step_output_keys(recipes: &[(String, String)]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (_, yaml) in recipes {
        let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("recipe parses as YAML");
        for s in doc["steps"].as_sequence().into_iter().flatten() {
            if let Some(k) = s.get("output").and_then(|v| v.as_str()) {
                keys.insert(k.to_string());
            }
        }
    }
    for k in RUN_INPUTS {
        keys.remove(*k);
    }
    keys
}

/// `(bare name, source line)` of every read of a step output through its bare
/// upper-case name in the bash text `body`. A read is `$NAME`, `${NAME…}`,
/// `${#NAME}` (length) or `${!NAME}` (indirect); a name the text itself
/// assigned on an earlier line is a local, not an inherited value. The runner
/// exports the bare alias only for scalars, and a step skipped by its
/// `condition:` leaves a parent run's value in place, so the only safe read is
/// `${RECIPE_VAR_<key>:-}`.
fn plain_bare_reads_in(body: &str, outputs: &BTreeSet<String>) -> Vec<(String, String)> {
    let read = Regex::new(r"\$(?:\{[#!]?)?([A-Z][A-Z0-9_]*)\b").unwrap();
    // `X=…` binds after the line's own reads; `for X in` before its body.
    let assign = Regex::new(
        r"(?:^|[\s;&|(])(?:(?:local|export|readonly|declare(?:\s+-[a-zA-Z]+)?)\s+)?([A-Z][A-Z0-9_]*)=|\bread\s+(?:-\S+\s+)*([A-Z][A-Z0-9_]*)",
    )
    .unwrap();
    let for_var = Regex::new(r"\bfor\s+([A-Z][A-Z0-9_]*)\s+in\b").unwrap();
    let mut locals = BTreeSet::new();
    let mut found = Vec::new();
    for raw in body.lines() {
        let code = strip_comment(raw);
        for c in for_var.captures_iter(code) {
            locals.insert(c[1].to_string());
        }
        for c in read.captures_iter(code) {
            let name = &c[1];
            if outputs.contains(&name.to_ascii_lowercase()) && !locals.contains(name) {
                found.push((name.to_string(), raw.trim().to_string()));
            }
        }
        for c in assign.captures_iter(code) {
            if let Some(m) = c.get(1).or_else(|| c.get(2)) {
                locals.insert(m.as_str().to_string());
            }
        }
    }
    found
}

/// `(step id, bare name, source line)` of every plain bare read of a step
/// output in a bash step of the recipe `yaml`. Locals are per step.
fn plain_bare_reads(yaml: &str, outputs: &BTreeSet<String>) -> Vec<(String, String, String)> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).expect("recipe parses as YAML");
    let mut found = Vec::new();
    for s in doc["steps"].as_sequence().into_iter().flatten() {
        let Some(body) = s.get("command").and_then(|v| v.as_str()) else {
            continue;
        };
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        for (name, line) in plain_bare_reads_in(body, outputs) {
            found.push((id.to_string(), name, line));
        }
    }
    found
}

/// Every plain bare read of a step output in the bundle: recipe steps by step
/// id, and tool scripts as a whole (step id `""`), since a step body moved
/// into a tool runs with the same environment. Keyed by
/// `(file name, step id, bare name)`, with the source lines.
fn bundle_plain_bare_reads() -> BTreeMap<(String, String, String), Vec<String>> {
    let recipes = recipe_files();
    let outputs = step_output_keys(&recipes);
    let mut reads: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let file_name = |path: &str| path.rsplit('/').next().unwrap().to_string();
    for (path, yaml) in &recipes {
        for (id, bare, line) in plain_bare_reads(yaml, &outputs) {
            reads
                .entry((file_name(path), id, bare))
                .or_default()
                .push(line);
        }
    }
    for (path, text) in tool_files() {
        for (bare, line) in plain_bare_reads_in(&text, &outputs) {
            reads
                .entry((file_name(&path), String::new(), bare))
                .or_default()
                .push(line);
        }
    }
    reads
}

#[test]
fn no_bundle_file_reads_a_step_output_through_a_plain_bare_name() {
    let reads = bundle_plain_bare_reads();
    let allowed: BTreeMap<(String, String, String), usize> = PLAIN_READS_NOT_YET_CONVERTED
        .iter()
        .map(|(f, id, bare, n)| ((f.to_string(), id.to_string(), bare.to_string()), *n))
        .collect();
    assert_eq!(
        allowed.len(),
        PLAIN_READS_NOT_YET_CONVERTED.len(),
        "PLAIN_READS_NOT_YET_CONVERTED lists a (file, step, name) twice"
    );
    let mut new_reads = Vec::new();
    for (key, lines) in &reads {
        let budget = allowed.get(key).copied().unwrap_or(0);
        if lines.len() > budget {
            let (file, id, bare) = key;
            new_reads.push(format!(
                "{file}: step {id:?}: {bare}: {} reads, {budget} allowed:\n    {}",
                lines.len(),
                lines.join("\n    ")
            ));
        }
    }
    assert!(
        new_reads.is_empty(),
        "{} new reads of a step output through its bare upper-case name (#1561, \
         #1684). Read it as \"${{RECIPE_VAR_<key>:-}}\" and declare <key> in the \
         recipe's `context:` unless an earlier unconditional step produces it:\n{}",
        new_reads.len(),
        new_reads.join("\n")
    );
    let stale: Vec<String> = allowed
        .iter()
        .filter(|(key, n)| reads.get(*key).map_or(0, Vec::len) < **n)
        .map(|((f, id, bare), n)| {
            let now = reads
                .get(&(f.clone(), id.clone(), bare.clone()))
                .map_or(0, Vec::len);
            format!("({f:?}, {id:?}, {bare:?}, {n}) now has {now}")
        })
        .collect();
    assert!(
        stale.is_empty(),
        "lower or remove these PLAIN_READS_NOT_YET_CONVERTED entries; they have fewer \
         plain bare reads than listed (or the file no longer exists):\n{}",
        stale.join("\n")
    );
}

#[test]
fn the_plain_read_detector_flags_step_outputs_and_skips_locals() {
    let yaml = r#"
steps:
  - id: "produce"
    type: "bash"
    command: "echo x"
    output: "issue_number"
  - id: "reads"
    type: "bash"
    command: |
      A="$ISSUE_NUMBER"
      bash tool --issue "${ISSUE_NUMBER:-}"
      echo "${RECIPE_VAR_issue_number:-}" "$REPO_PATH"
      # was "$ISSUE_NUMBER"
      echo "${#ISSUE_NUMBER}" "${!ISSUE_NUMBER}" "$#ISSUE_NUMBER"
  - id: "local"
    type: "bash"
    command: |
      ISSUE_NUMBER=7
      echo "$ISSUE_NUMBER"
  - id: "loop"
    type: "bash"
    command: |
      for ISSUE_NUMBER in 1 2; do echo "$ISSUE_NUMBER"; done
"#;
    let outputs = step_output_keys(&[(String::new(), yaml.to_string())]);
    let got: Vec<(String, String)> = plain_bare_reads(yaml, &outputs)
        .into_iter()
        .map(|(id, bare, _)| (id, bare))
        .collect();
    // `$#ISSUE_NUMBER` is `$#` followed by text, not a read.
    let want = vec![("reads".to_string(), "ISSUE_NUMBER".to_string()); 4];
    assert_eq!(got, want);

    // A tool script has no steps: the whole file is one body, so an
    // assignment anywhere earlier makes the name a local.
    let tool = "ISSUE=\"${ISSUE_NUMBER:-}\"\nLOCAL_TESTING_GATE=x\necho \"$LOCAL_TESTING_GATE\" \"${#ISSUE_NUMBER}\"\n";
    let outputs: BTreeSet<String> = ["issue_number", "local_testing_gate"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let got: Vec<String> = plain_bare_reads_in(tool, &outputs)
        .into_iter()
        .map(|(bare, _)| bare)
        .collect();
    assert_eq!(got, vec!["ISSUE_NUMBER", "ISSUE_NUMBER"]);
}
