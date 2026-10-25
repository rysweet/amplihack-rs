//! Issues #1561 / #1607 — a recipe's own step output must win over a bare
//! upper-case variable inherited from whatever launched the runner.
//!
//! The bundle used to read step outputs as `${UPPER:-${RECIPE_VAR_lower:-}}`.
//! The pinned recipe runner (`claude-plugin/recipe-runner.rev`) always exports
//! `RECIPE_VAR_<key>`, adds the bare upper-case alias only for top-level
//! scalars, and builds each step's environment on top of the runner's own
//! inherited one. So a stale `QA_EVIDENCE` or `AUTODRIVE_TOOLS_DIR` from a
//! parent recipe run, a shell or a CI job reached the step and outranked the
//! value the current recipe produced, silently. #1607 is the observed case: a
//! nested auto-drive round took the outer run's tools directory (PR #1588's QA
//! case `MERGEREADY-no-round-tool`).
//!
//! The fix drops the bare read: every step output is read as
//! `${RECIPE_VAR_<lower>:-<default>}`, in every recipe and every bundle tool.
//! This guard fails if any spelling in which a bare name can win comes back,
//! if a bare name is left anywhere after a RECIPE_VAR_ read in a chain, or if
//! a recipe reads a RECIPE_VAR_ key it does not declare (the runner replaces
//! only declared keys, so an undeclared one is the parent's value). The one
//! kept bare name is `NO_MERGE`, as an explicit override that can only switch
//! merging off. See "Step outputs ignore inherited bare variables" in
//! `docs/reference/auto-drive-to-merge.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Not recursive either: `tools/gh-merge-guard/` is the #1551 shim, not a
/// step-output reader. Every `*.sh` here is scanned: the merge round's
/// evidence reads and the workflow tools read step outputs the same way.
const TOOLS_DIR: &str = "amplifier-bundle/tools";

/// Floors that stop the scan from silently covering nothing.
const MIN_RECIPES: usize = 20;
const MIN_TOOLS: usize = 40;

/// The docs section and anchor the `NO_MERGE` override points at.
const DOCS_PAGE: &str = "docs/reference/auto-drive-to-merge.md";
const NO_MERGE_ANCHOR: &str = "no_merge-override";

/// Bare names that are deliberate, documented overrides, as
/// `(NAME, "documented in docs/<page>.md#<anchor>", justification)`. The page
/// must exist, mention `NAME` and carry the anchor; the justification must say
/// why the bare name may be read at all. An allowed name is exempt only in
/// its exact override shape (see `override_shape`), never as a fallback.
const ALLOWED_OVERRIDES: &[(&str, &str, &str)] = &[(
    "NO_MERGE",
    "documented in docs/reference/auto-drive-to-merge.md#no_merge-override",
    "an operator's no-merge request; it is OR'd in after the recipe's no_merge \
     is resolved, so a stale value can only stop a merge, never cause one",
)];

/// The only shape in which an allowed override may read its bare name.
const OVERRIDE_MARKER: &str = "override (#1561)";
const OVERRIDE_INFO: &str = "bare NO_MERGE env";

/// The three steps that apply the `NO_MERGE` override (#1561 D2).
const NO_MERGE_SITES: &[(&str, &str)] = &[
    (
        "amplifier-bundle/recipes/autodrive-build.yaml",
        "step-01b-no-merge-directive",
    ),
    (
        "amplifier-bundle/recipes/autodrive-build.yaml",
        "step-03-resolve-pr",
    ),
    (
        "amplifier-bundle/recipes/workflow-terminal-state.yaml",
        "probe-terminal-state",
    ),
];

/// The test-only opt-out that keeps the caller's `AUTODRIVE_*` and
/// `RECIPE_VAR_*` variables in the shell contract test.
const KEEP_INHERITED: &str = "AUTODRIVE_TEST_KEEP_INHERITED";

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

/// GitHub-style anchor of a markdown heading.
fn anchor_of(heading: &str) -> String {
    heading
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter_map(|c| match c {
            'a'..='z' | '0'..='9' | '_' | '-' => Some(c),
            ' ' => Some('-'),
            _ => None,
        })
        .collect()
}

/// Checks one allowlist entry; `Err` names what is wrong.
fn check_override(root: &Path, name: &str, reason: &str, why: &str) -> Result<(), String> {
    let Some(doc) = reason.strip_prefix("documented in ") else {
        return Err(format!(
            "{name}: reason `{reason}` must start with `documented in <doc>#<anchor>`"
        ));
    };
    let (doc, anchor) = match doc.trim().split_once('#') {
        Some((d, a)) => (d, a),
        None => return Err(format!("{name}: `{doc}` names no #anchor")),
    };
    if !doc.starts_with("docs/") || doc.contains("..") {
        return Err(format!("{name}: `{doc}` is not a page under docs/"));
    }
    let Ok(text) = std::fs::read_to_string(root.join(doc)) else {
        return Err(format!("{name}: `{doc}` does not exist"));
    };
    if !text.contains(name) {
        return Err(format!("{name}: `{doc}` does not mention {name}"));
    }
    let has_anchor = text
        .lines()
        .filter_map(|l| l.strip_prefix('#'))
        .map(|l| l.trim_start_matches('#'))
        .any(|h| anchor_of(h) == anchor);
    if !has_anchor {
        return Err(format!(
            "{name}: `{doc}` has no heading with anchor #{anchor}"
        ));
    }
    if why.split_whitespace().count() < 8 {
        return Err(format!(
            "{name}: justification `{why}` is too thin to review"
        ));
    }
    Ok(())
}

/// Whether line `idx` of `lines` is the one permitted shape of the `NO_MERGE`
/// override: a separate statement, after the recipe value is resolved, that
/// passes `${NO_MERGE:-}` through `normalise_no_merge_flag`, compares the
/// result with `true` and only then switches no-merge on; with an
/// `override (#1561)` comment linking the docs anchor on one of the two lines
/// above, and an INFO line naming the bare NO_MERGE env within three lines.
/// `Err` names what is wrong.
fn override_shape(lines: &[&str], idx: usize) -> Result<(), String> {
    let line = lines[idx].trim();
    let reads: Vec<_> = Regex::new(r"\$\{?NO_MERGE\b(?:[^_A-Za-z0-9]|$)")
        .unwrap()
        .find_iter(line)
        .collect();
    if reads.len() != 1 {
        return Err(format!("{} bare NO_MERGE reads on one line", reads.len()));
    }
    if !line.contains("normalise_no_merge_flag \"${NO_MERGE:-}\"") {
        return Err(
            "the bare NO_MERGE must be read as `normalise_no_merge_flag \"${NO_MERGE:-}\"`, \
             with an empty default"
                .into(),
        );
    }
    if line.contains("RECIPE_VAR_") {
        return Err(
            "the override must be its own statement, not part of a RECIPE_VAR_ chain".into(),
        );
    }
    let compare =
        Regex::new(r#"normalise_no_merge_flag "\$\{NO_MERGE:-\}"\)"?\s*=\s*"?true"?\s*\]\]?\s*&&"#)
            .unwrap();
    if !compare.is_match(line) {
        return Err("the override must test `= true` and act only with `&&`".into());
    }
    if Regex::new(r"(?i)\bfalse\b|\|\||!=").unwrap().is_match(line) {
        return Err("the override line may only switch no-merge on".into());
    }
    let from = idx.saturating_sub(2);
    let commented = lines[from..idx].iter().any(|l| {
        let l = l.trim();
        l.starts_with('#')
            && l.contains(OVERRIDE_MARKER)
            && l.contains(&format!("auto-drive-to-merge.md#{NO_MERGE_ANCHOR}"))
    }) || lines[from..idx].windows(2).any(|w| {
        let joined = format!("{} {}", w[0].trim(), w[1].trim());
        w[0].trim().starts_with('#')
            && w[1].trim().starts_with('#')
            && joined.contains(OVERRIDE_MARKER)
            && joined.contains(&format!("auto-drive-to-merge.md#{NO_MERGE_ANCHOR}"))
    });
    if !commented {
        return Err(format!(
            "no `# {OVERRIDE_MARKER}` comment linking {DOCS_PAGE}#{NO_MERGE_ANCHOR} on the \
             two lines above"
        ));
    }
    let to = (idx + 4).min(lines.len());
    if !lines[idx..to].iter().any(|l| l.contains(OVERRIDE_INFO)) {
        return Err(format!(
            "no INFO line naming `{OVERRIDE_INFO}` within three lines"
        ));
    }
    Ok(())
}

/// Every non-comment line of `text` that reads the bare `NO_MERGE`.
fn no_merge_reads(text: &str) -> Vec<usize> {
    let re = Regex::new(r"\$\{?NO_MERGE\b(?:[^_A-Za-z0-9]|$)").unwrap();
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#') && re.is_match(l))
        .map(|(i, _)| i)
        .collect()
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

/// The `normalise_no_merge_flag` function a recipe step defines, as bash.
fn normaliser_in(step_body: &str) -> Option<String> {
    let lines: Vec<&str> = step_body.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("normalise_no_merge_flag()"))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let end = lines[start..]
        .iter()
        .position(|l| l.trim() == "}" && l.len() - l.trim_start().len() == indent)?;
    Some(lines[start..=start + end].join("\n"))
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

// ── The guard ────────────────────────────────────────────────────────────────

#[test]
fn no_bundle_read_lets_a_bare_name_win_over_recipe_var() {
    let allowed: Vec<&str> = ALLOWED_OVERRIDES.iter().map(|(n, _, _)| *n).collect();
    let mut report = Vec::new();
    for (path, text) in scanned_files() {
        let lines: Vec<&str> = text.lines().collect();
        for v in scan(&text) {
            let in_shape =
                allowed.contains(&v.bare.as_str()) && override_shape(&lines, v.line - 1).is_ok();
            if !in_shape {
                report.push(v.render(&path));
            }
        }
    }
    assert!(
        report.is_empty(),
        "{} step-output reads let an inherited bare variable win over the recipe's \
         own RECIPE_VAR_ value (#1561, #1607):\n{}\n\n\
         Read the step output as \"${{RECIPE_VAR_<lower>:-<default>}}\" and drop the \
         bare name. Do not move the bare name after the RECIPE_VAR_ read either: an \
         empty step output would then fall through to the stale value. See \"Step \
         outputs ignore inherited bare variables\" in docs/reference/auto-drive-to-merge.md.",
        report.len(),
        report.join("\n")
    );
}

#[test]
fn every_allowed_override_is_documented() {
    let root = repo_root();
    let bad: Vec<String> = ALLOWED_OVERRIDES
        .iter()
        .filter_map(|(name, reason, why)| check_override(&root, name, reason, why).err())
        .collect();
    assert!(
        bad.is_empty(),
        "ALLOWED_OVERRIDES entries without a real justification:\n{}",
        bad.join("\n")
    );
    let names: Vec<&str> = ALLOWED_OVERRIDES.iter().map(|(n, _, _)| *n).collect();
    assert_eq!(
        names,
        ["NO_MERGE"],
        "NO_MERGE is the only documented override (#1561 D2); every other bare name is \
         set with `amplihack recipe run -c <key>=…`, not inherited"
    );
}

#[test]
fn the_allowlist_check_fails_closed() {
    let root = repo_root();
    let why = "an operator's no-merge request that can only ever stop a merge";
    let ok = "documented in docs/reference/auto-drive-to-merge.md#no_merge-override";
    for (name, reason, why, what) in [
        ("NO_MERGE", "because", why, "no `documented in`"),
        (
            "NO_MERGE",
            "documented in docs/no-such.md#x",
            why,
            "a missing page",
        ),
        (
            "NO_MERGE",
            "documented in README.md#x",
            why,
            "a page outside docs/",
        ),
        (
            "NO_MERGE",
            "documented in docs/../Cargo.toml#x",
            why,
            "a path escaping docs/",
        ),
        (
            "NO_MERGE",
            "documented in docs/reference/auto-drive-to-merge.md",
            why,
            "no anchor",
        ),
        (
            "NO_MERGE",
            "documented in docs/reference/auto-drive-to-merge.md#no-such-anchor-1561",
            why,
            "an anchor the page lacks",
        ),
        (
            "NO_SUCH_VARIABLE_1561",
            ok,
            why,
            "a page that does not mention the name",
        ),
        ("NO_MERGE", ok, "", "an empty justification"),
        ("NO_MERGE", ok, "safety", "a one-word justification"),
    ] {
        assert!(
            check_override(&root, name, reason, why).is_err(),
            "{what} must not count as a justification"
        );
    }
    assert!(check_override(&root, "NO_MERGE", ok, why).is_ok());
    assert_eq!(anchor_of(" NO_MERGE override"), "no_merge-override");
    assert_eq!(
        anchor_of("Step outputs ignore inherited bare variables"),
        "step-outputs-ignore-inherited-bare-variables"
    );
}

// ── The NO_MERGE override (#1561 D2) ─────────────────────────────────────────

const GOOD_OVERRIDE: &str = "NM=\"$(normalise_no_merge_flag \"${RECIPE_VAR_no_merge:-}\")\"
# override (#1561): a bare NO_MERGE can only add a no-merge; see
# docs/reference/auto-drive-to-merge.md#no_merge-override
[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" = \"true\" ] && { NM=true; echo \"INFO: no-merge from the bare NO_MERGE env\" >&2; }
";

fn shape_of(text: &str) -> Result<(), String> {
    let lines: Vec<&str> = text.lines().collect();
    let reads = no_merge_reads(text);
    assert_eq!(reads.len(), 1, "fixture must read NO_MERGE once: {text}");
    override_shape(&lines, reads[0])
}

#[test]
fn no_merge_override_can_only_tighten() {
    shape_of(GOOD_OVERRIDE).unwrap_or_else(|e| panic!("the canonical override is refused: {e}"));
    assert!(scan(GOOD_OVERRIDE).is_empty(), "{:?}", scan(GOOD_OVERRIDE));
    let single_comment = "# override (#1561): see docs/reference/auto-drive-to-merge.md#no_merge-override\n\
         [[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" == \"true\" ]] 2>/dev/null; :\n";
    assert!(
        shape_of(single_comment).is_err(),
        "`==` without `&&` is not the shape"
    );

    let comment =
        "# override (#1561): see docs/reference/auto-drive-to-merge.md#no_merge-override\n";
    let info = "\necho \"INFO: bare NO_MERGE env\" >&2\n";
    for (body, what) in [
        (
            "NM=\"${NO_MERGE:-${RECIPE_VAR_no_merge:-}}\"",
            "nested: the bare name first",
        ),
        (
            "NM=\"${RECIPE_VAR_no_merge:-${NO_MERGE:-}}\"",
            "bare-last: an empty recipe value falls through",
        ),
        (
            "[ \"$(normalise_no_merge_flag \"${NO_MERGE:-false}\")\" = \"true\" ] && NM=true",
            "':-false' default",
        ),
        (
            "[ \"$(normalise_no_merge_flag \"${NO_MERGE:=}\")\" = \"true\" ] && NM=true",
            "':=' assignment",
        ),
        (
            "[ \"${NO_MERGE:-}\" = \"true\" ] && NM=true",
            "no normaliser",
        ),
        (
            "NM=\"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\"",
            "an assignment that can set false",
        ),
        (
            "[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" = \"true\" ] && NM=true || NM=false",
            "a branch that lifts no-merge",
        ),
        (
            "[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" != \"true\" ] && NM=false",
            "a negated test that lifts no-merge",
        ),
        (
            "[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" = \"true\" ] && NM=\"${RECIPE_VAR_no_merge:-}\"",
            "part of a RECIPE_VAR_ chain",
        ),
    ] {
        let text = format!("{comment}{body}{info}");
        assert!(shape_of(&text).is_err(), "{what} must be refused: {text}");
    }
    let body = "[ \"$(normalise_no_merge_flag \"${NO_MERGE:-}\")\" = \"true\" ] && NM=true";
    assert!(
        shape_of(&format!("{body}{info}")).is_err(),
        "an override with no comment must be refused"
    );
    assert!(
        shape_of(&format!("{comment}x=1\ny=2\n{body}{info}")).is_err(),
        "a comment three lines up is too far"
    );
    assert!(
        shape_of(&format!("# override (#1561)\n{body}{info}")).is_err(),
        "a comment without the docs anchor must be refused"
    );
    assert!(
        shape_of(&format!("{comment}{body}\n")).is_err(),
        "an override with no INFO line must be refused"
    );
    // In the guard, an allowlisted name in a bad shape is still a violation.
    let lines: Vec<&str> = "X=1\nNM=\"${NO_MERGE:-${RECIPE_VAR_no_merge:-}}\"\n"
        .lines()
        .collect();
    let v = scan(&lines.join("\n"));
    assert!(v.iter().any(|v| v.bare == "NO_MERGE"), "{v:?}");
    assert!(override_shape(&lines, v[0].line - 1).is_err());
}

/// The real bundle: every bare NO_MERGE read is the override shape, and the
/// override sits at exactly the three steps that decide no-merge.
#[test]
fn the_bundle_reads_no_merge_only_as_the_override() {
    let mut sites = Vec::new();
    let mut bad = Vec::new();
    for (path, text) in scanned_files() {
        let lines: Vec<&str> = text.lines().collect();
        for i in no_merge_reads(&text) {
            match override_shape(&lines, i) {
                Ok(()) => sites.push(path.clone()),
                Err(e) => bad.push(format!("{path}:{}: {e}: {}", i + 1, lines[i].trim())),
            }
        }
    }
    assert!(
        bad.is_empty(),
        "bare NO_MERGE reads outside the override shape (#1561 D2):\n{}",
        bad.join("\n")
    );
    let root = repo_root();
    for (file, id) in NO_MERGE_SITES {
        let yaml = std::fs::read_to_string(root.join(file)).expect("read recipe");
        let body = step_body(&yaml, id).unwrap_or_else(|| panic!("{file} has no step {id}"));
        let lines: Vec<&str> = body.lines().collect();
        let here: Vec<usize> = no_merge_reads(&body);
        assert_eq!(
            here.len(),
            1,
            "{file} {id} must apply the NO_MERGE override exactly once (#1561 D2)"
        );
        override_shape(&lines, here[0]).unwrap_or_else(|e| panic!("{file} {id}: {e}"));
        assert!(
            body.contains("normalise_no_merge_flag \"${RECIPE_VAR_no_merge:-}\"")
                || body.contains("normalise_no_merge_flag \"$NO_MERGE_FLAG_INPUT\""),
            "{file} {id}: the recipe value must go through the same normaliser as the bare one (S1)"
        );
        let first_bare = here[0];
        let first_recipe = lines
            .iter()
            .position(|l| !l.trim_start().starts_with('#') && l.contains("RECIPE_VAR_no_merge"))
            .unwrap_or_else(|| panic!("{file} {id} never reads RECIPE_VAR_no_merge"));
        assert!(
            first_recipe < first_bare,
            "{file} {id}: the bare NO_MERGE must be OR'd in after the recipe value is resolved"
        );
    }
    assert_eq!(
        sites.len(),
        NO_MERGE_SITES.len(),
        "the NO_MERGE override must appear at exactly the {} known sites; found it in {sites:?}",
        NO_MERGE_SITES.len()
    );
}

/// Each override site's normaliser: trimmed, case-insensitive, true|1|yes|on.
#[test]
fn every_no_merge_normaliser_accepts_the_same_truthy_tokens() {
    let root = repo_root();
    for (file, id) in NO_MERGE_SITES {
        let yaml = std::fs::read_to_string(root.join(file)).expect("read recipe");
        let body = step_body(&yaml, id).unwrap_or_else(|| panic!("{file} has no step {id}"));
        let func = normaliser_in(&body)
            .unwrap_or_else(|| panic!("{file} {id} defines no normalise_no_merge_flag() (S1)"));
        for (input, truthy) in [
            ("true", true),
            ("TRUE", true),
            (" On ", true),
            ("yes", true),
            ("YES\n", true),
            ("1", true),
            ("on", true),
            ("false", false),
            ("", false),
            ("0", false),
            ("no", false),
            ("off", false),
            ("maybe", false),
            ("truee", false),
        ] {
            let out = Command::new("bash")
                .arg("-c")
                .arg(format!("{func}\nnormalise_no_merge_flag \"$1\""))
                .arg("normaliser")
                .arg(input)
                .output()
                .expect("run bash");
            let got = String::from_utf8_lossy(&out.stdout).trim() == "true";
            assert_eq!(
                got,
                truthy,
                "{file} {id}: normalise_no_merge_flag {input:?} gave {:?}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}

/// The normaliser runs under macOS's `/bin/bash` 3.2 too. There `${v,,}` is a
/// "bad substitution": it prints nothing, so a recipe `no_merge=true` reads as
/// false and the run merges. It must not build a command from its input either.
#[test]
fn every_no_merge_normaliser_is_bash_3_2_safe() {
    let root = repo_root();
    let forbidden = [
        (
            Regex::new(r"\$\{[^}]*(,,|\^\^|,|\^)\}").unwrap(),
            "case-modifying expansion (bash 4+)",
        ),
        (
            Regex::new(r"\b(declare|typeset|local)\s+-[a-zA-Z]*[lun]").unwrap(),
            "declare -l/-u/-n (bash 4+, or a nameref)",
        ),
        (Regex::new(r"\beval\b").unwrap(), "eval"),
        (Regex::new(r"\$\{!").unwrap(), "indirect expansion"),
    ];
    for (file, id) in NO_MERGE_SITES {
        let yaml = std::fs::read_to_string(root.join(file)).expect("read recipe");
        let body = step_body(&yaml, id).unwrap_or_else(|| panic!("{file} has no step {id}"));
        let func = normaliser_in(&body)
            .unwrap_or_else(|| panic!("{file} {id} defines no normalise_no_merge_flag() (S1)"));
        for (re, what) in &forbidden {
            if let Some(m) = re.find(&func) {
                panic!(
                    "{file} {id}: normalise_no_merge_flag uses {what} (`{}`); it must run under bash 3.2",
                    m.as_str()
                );
            }
        }
    }
    // The detector itself: the bash 4 spelling is caught.
    assert!(forbidden[0].0.is_match(r#"v="${1,,}""#));
    assert!(!forbidden[0].0.is_match(r#"v="${1:-}""#));
}

// ── Whole-tree assertions ────────────────────────────────────────────────────

/// The success criterion as a plain regex, independent of the detectors.
#[test]
fn no_bare_name_defaults_to_a_recipe_var_anywhere() {
    let re = Regex::new(r"\$\{[A-Z_][A-Z0-9_]*:-\$\{RECIPE_VAR_").unwrap();
    let hits: Vec<String> = scanned_files()
        .iter()
        .flat_map(|(path, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, l)| re.is_match(l))
                .map(|(i, l)| format!("{path}:{}: {}", i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "{} reads still put a bare name before RECIPE_VAR_ (#1561):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

/// #1607: the tools directory decides which code runs. Its only source is
/// step-00's validated `RECIPE_VAR_autodrive_tools_dir`.
#[test]
fn nothing_in_the_bundle_reads_autodrive_tools_dir() {
    let re = Regex::new(r"\$\{?AUTODRIVE_TOOLS_DIR\b").unwrap();
    let hits: Vec<String> = scanned_files()
        .iter()
        .flat_map(|(path, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, l)| !l.trim_start().starts_with('#') && re.is_match(l))
                .map(|(i, l)| format!("{path}:{}: {}", i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "the bundle still reads the inherited AUTODRIVE_TOOLS_DIR (#1607):\n{}",
        hits.join("\n")
    );
}

/// The merge round refuses an empty tools dir before it builds a path: with
/// `D` empty, `"${D}/autodrive_x.sh"` is `/autodrive_x.sh` at the root (S3).
#[test]
fn every_merge_round_tools_dir_read_fails_closed_when_empty() {
    let root = repo_root();
    let yaml = std::fs::read_to_string(root.join(RECIPES_DIR).join("autodrive-merge-round.yaml"))
        .expect("read autodrive-merge-round.yaml");
    let read = Regex::new(r#"^\s*([A-Z_][A-Z0-9_]*)="\$\{RECIPE_VAR_autodrive_tools_dir:-\}"\s*$"#)
        .unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let mut checked = 0;
    let mut bad = Vec::new();
    for s in doc["steps"].as_sequence().unwrap() {
        let id = s["id"].as_str().unwrap_or("?");
        let Some(body) = s.get("command").and_then(|c| c.as_str()) else {
            continue;
        };
        let lines: Vec<&str> = body.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.trim_start().starts_with('#') || !l.contains("RECIPE_VAR_autodrive_tools_dir") {
                continue;
            }
            checked += 1;
            let Some(c) = read.captures(l) else {
                bad.push(format!(
                    "{id}: read the tools dir into a local first, as \
                     `D=\"${{RECIPE_VAR_autodrive_tools_dir:-}}\"`: {}",
                    l.trim()
                ));
                continue;
            };
            let d = regex::escape(&c[1]);
            // `[ -n "$D" ] || …`, or `[ -n "$D" ] && [ -d "$D" ] || …` on one line.
            let guard = Regex::new(&format!(
                r#"\[\[?\s+-n\s+"\$\{{?{d}\}}?"\s*\]\]?\s*(?:&&\s*\[\[?\s+-d\s+"\$\{{?{d}\}}?"\s*\]\]?\s*)?\|\|.*autodrive-tools-not-found.*exit\s+[1-9]"#
            ))
            .unwrap();
            if !lines.get(i + 1).is_some_and(|n| guard.is_match(n)) {
                bad.push(format!(
                    "{id}: the line after `{}` must be `[ -n \"${}\" ] || {{ echo \"ERROR: \
                     autodrive-tools-not-found: …\" >&2; exit 1; }}`",
                    l.trim(),
                    &c[1]
                ));
            }
        }
    }
    assert!(
        checked >= 6,
        "only {checked} RECIPE_VAR_autodrive_tools_dir reads found in the merge round's steps; \
         the 6 tools-resolving steps must each read it"
    );
    assert!(bad.is_empty(), "#1561 S3:\n{}", bad.join("\n"));
}

/// Each tools-resolving step of the merge round, as (step id, body).
fn merge_round_tool_steps() -> Vec<(String, String)> {
    let yaml = std::fs::read_to_string(
        repo_root()
            .join(RECIPES_DIR)
            .join("autodrive-merge-round.yaml"),
    )
    .expect("read autodrive-merge-round.yaml");
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|s| {
            let body = s.get("command")?.as_str()?;
            body.lines()
                .any(|l| {
                    !l.trim_start().starts_with('#') && l.contains("RECIPE_VAR_autodrive_tools_dir")
                })
                .then(|| {
                    (
                        s["id"].as_str().unwrap_or("?").to_string(),
                        body.to_string(),
                    )
                })
        })
        .collect()
}

/// The tools local a step reads `RECIPE_VAR_autodrive_tools_dir` into.
fn tools_local(body: &str) -> Option<String> {
    let read = Regex::new(r#"^\s*([A-Z_][A-Z0-9_]*)="\$\{RECIPE_VAR_autodrive_tools_dir:-\}"\s*$"#)
        .unwrap();
    body.lines()
        .find_map(|l| read.captures(l).map(|c| c[1].to_string()))
}

/// SR1.2 for one step: the tools-dir read, its `-n` guard, then a `-d` guard
/// on the next line, each naming `autodrive-tools-not-found`.
fn tools_dir_problems(id: &str, body: &str) -> Vec<String> {
    let Some(d) = tools_local(body) else {
        return vec![format!(
            "{id}: reads the tools dir without a `D=\"${{RECIPE_VAR_autodrive_tools_dir:-}}\"` local"
        )];
    };
    if d == "AUTODRIVE_TOOLS_DIR" {
        return vec![format!(
            "{id}: the local must not reuse the inherited name AUTODRIVE_TOOLS_DIR (#1607)"
        )];
    }
    let de = regex::escape(&d);
    let dir_check = Regex::new(&format!(
        r#"\[\[?\s+-d\s+"\$\{{?{de}\}}?"\s*\]\]?\s*\|\|.*autodrive-tools-not-found.*exit\s+[1-9]"#
    ))
    .unwrap();
    let both_check = Regex::new(&format!(
        r#"\[\[?\s+-n\s+"\$\{{?{de}\}}?"\s*\]\]?\s*&&\s*\[\[?\s+-d\s+"\$\{{?{de}\}}?"\s*\]\]?\s*\|\|.*autodrive-tools-not-found.*exit\s+[1-9]"#
    ))
    .unwrap();
    let lines: Vec<&str> = body.lines().collect();
    let at = lines
        .iter()
        .position(|l| {
            !l.trim_start().starts_with('#') && l.contains("RECIPE_VAR_autodrive_tools_dir")
        })
        .unwrap();
    if lines.get(at + 1).is_some_and(|l| both_check.is_match(l))
        || lines.get(at + 2).is_some_and(|l| dir_check.is_match(l))
    {
        Vec::new()
    } else {
        vec![format!(
            "{id}: the line after the tools-dir read must be `[ -n \"${d}\" ] && [ -d \"${d}\" ] || \
             {{ echo \"ERROR: autodrive-tools-not-found: …\" >&2; exit 1; }}`, or a `-n` check and \
             then a `[ -d \"${d}\" ] || …` check"
        )]
    }
}

/// SR1 for one step: (tool uses seen, uses with no earlier `[ -f ]` check).
fn unchecked_tool_uses(id: &str, body: &str) -> (usize, Vec<String>) {
    let Some(d) = tools_local(body) else {
        return (0, vec![format!("{id}: no tools-dir local")]);
    };
    let de = regex::escape(&d);
    let path = Regex::new(&format!(r#"\$\{{?{de}\}}?/([A-Za-z0-9_.-]+\.sh)"#)).unwrap();
    let alias = Regex::new(&format!(
        r#"^\s*([A-Za-z_][A-Za-z0-9_]*)="\$\{{?{de}\}}?/([A-Za-z0-9_.-]+\.sh)"\s*$"#
    ))
    .unwrap();
    let f_check = Regex::new(r#"\[\[?\s+-f\s+"#).unwrap();
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let mut checked: BTreeSet<String> = BTreeSet::new();
    let mut seen = 0;
    let mut bad = Vec::new();
    for (i, l) in body.lines().enumerate() {
        if l.trim_start().starts_with('#') {
            continue;
        }
        if let Some(c) = alias.captures(l) {
            aliases.insert(c[1].to_string(), c[2].to_string());
            continue;
        }
        let mut named: Vec<String> = path.captures_iter(l).map(|c| c[1].to_string()).collect();
        for (a, t) in &aliases {
            if l.contains(&format!("\"${a}\"")) || l.contains(&format!("\"${{{a}}}\"")) {
                named.push(t.clone());
            }
        }
        if f_check.is_match(l) {
            checked.extend(named);
            continue;
        }
        for t in named {
            seen += 1;
            if !checked.contains(&t) {
                bad.push(format!(
                    "{id}:{}: `{t}` is used before a `[ -f ]` check: {}",
                    i + 1,
                    l.trim()
                ));
            }
        }
    }
    (seen, bad)
}

/// SR1.2: a set but missing tools dir is refused by name too, before any path
/// is built from it. The `-n` guard alone lets `/nonexistent/x.sh` through to
/// a `[ -f ]` check that names the file, not the tools dir.
#[test]
fn every_merge_round_tools_dir_read_checks_the_directory_exists() {
    let steps = merge_round_tool_steps();
    assert!(
        steps.len() >= 6,
        "only {} tools-resolving steps in the merge round",
        steps.len()
    );
    let bad: Vec<String> = steps
        .iter()
        .flat_map(|(id, body)| tools_dir_problems(id, body))
        .collect();
    assert!(bad.is_empty(), "#1607 SR1.2:\n{}", bad.join("\n"));
}

/// SR1: every tool the merge round sources or runs from the tools dir has a
/// `[ -f ]` check first, so a dir without the tool fails by name instead of
/// sourcing nothing (`. missing` under `|| G=""`) or running a stray path.
#[test]
fn every_tool_the_merge_round_runs_is_checked_first() {
    let mut seen = 0;
    let mut bad = Vec::new();
    for (id, body) in merge_round_tool_steps() {
        let (n, b) = unchecked_tool_uses(&id, &body);
        seen += n;
        bad.extend(b);
    }
    assert!(bad.is_empty(), "#1607 SR1:\n{}", bad.join("\n"));
    assert!(
        seen >= 6,
        "only {seen} tool uses found in the merge round's steps"
    );
}

const GOOD_TOOLS_STEP: &str = r#"set -uo pipefail
TOOLS_D="${RECIPE_VAR_autodrive_tools_dir:-}"
[ -n "$TOOLS_D" ] || { echo "ERROR: autodrive-tools-not-found: RECIPE_VAR_autodrive_tools_dir is empty" >&2; exit 1; }
[ -d "$TOOLS_D" ] || { echo "ERROR: autodrive-tools-not-found: the tools dir is not a directory" >&2; exit 1; }
T="${TOOLS_D}/autodrive_round_evidence.sh"
[ -f "$T" ] || { echo "ERROR: autodrive-tools-not-found: ${T}" >&2; exit 1; }
bash "$T" crusty-evidence
[ -f "${TOOLS_D}/autodrive_trust.sh" ] || { echo "ERROR: autodrive-tools-not-found: autodrive_trust.sh" >&2; exit 1; }
G="$(. "${TOOLS_D}/autodrive_trust.sh" && autodrive_gadugi_required .)" || G=""
"#;

#[test]
fn the_tools_dir_checks_accept_the_fixed_step_and_refuse_each_gap() {
    assert_eq!(
        tools_dir_problems("good", GOOD_TOOLS_STEP),
        Vec::<String>::new()
    );
    let one_line = GOOD_TOOLS_STEP.replace(
        "[ -n \"$TOOLS_D\" ] || { echo \"ERROR: autodrive-tools-not-found: RECIPE_VAR_autodrive_tools_dir is empty\" >&2; exit 1; }\n\
         [ -d \"$TOOLS_D\" ] || { echo \"ERROR: autodrive-tools-not-found: the tools dir is not a directory\" >&2; exit 1; }\n",
        "[ -n \"$TOOLS_D\" ] && [ -d \"$TOOLS_D\" ] || { echo \"ERROR: autodrive-tools-not-found: autodrive_tools_dir is empty or not a directory\" >&2; exit 1; }\n",
    );
    assert_ne!(
        one_line, GOOD_TOOLS_STEP,
        "the one-line fixture must differ"
    );
    assert_eq!(
        tools_dir_problems("one-line", &one_line),
        Vec::<String>::new()
    );
    let one_line_no_d = one_line.replace(" && [ -d \"$TOOLS_D\" ]", "");
    assert_eq!(
        tools_dir_problems("one-line-no-d", &one_line_no_d).len(),
        1,
        "a one-line check without -d must be refused"
    );
    assert_eq!(
        unchecked_tool_uses("good", GOOD_TOOLS_STEP),
        (2, Vec::new())
    );

    let no_d = GOOD_TOOLS_STEP.replace(
        "[ -d \"$TOOLS_D\" ] || { echo \"ERROR: autodrive-tools-not-found: the tools dir is not a directory\" >&2; exit 1; }\n",
        "",
    );
    assert_eq!(
        tools_dir_problems("no-d", &no_d).len(),
        1,
        "a missing -d check must be refused"
    );
    let d_no_exit =
        GOOD_TOOLS_STEP.replace("not a directory\" >&2; exit 1;", "not a directory\" >&2;");
    assert_eq!(
        tools_dir_problems("d-no-exit", &d_no_exit).len(),
        1,
        "a -d check that does not exit must be refused"
    );
    let old_name = GOOD_TOOLS_STEP.replace("TOOLS_D", "AUTODRIVE_TOOLS_DIR");
    assert_eq!(
        tools_dir_problems("old-name", &old_name).len(),
        1,
        "the inherited name as the local must be refused"
    );
    let bare = GOOD_TOOLS_STEP.replace(
        "\"${RECIPE_VAR_autodrive_tools_dir:-}\"",
        "\"${AUTODRIVE_TOOLS_DIR:-${RECIPE_VAR_autodrive_tools_dir:-}}\"",
    );
    assert_eq!(
        tools_dir_problems("bare", &bare).len(),
        1,
        "the dual-name read must be refused"
    );

    let no_f_source = GOOD_TOOLS_STEP.replace(
        "[ -f \"${TOOLS_D}/autodrive_trust.sh\" ] || { echo \"ERROR: autodrive-tools-not-found: autodrive_trust.sh\" >&2; exit 1; }\n",
        "",
    );
    let (_, bad) = unchecked_tool_uses("no-f-source", &no_f_source);
    assert_eq!(
        bad.len(),
        1,
        "a sourced tool with no -f check must be refused: {bad:?}"
    );
    let no_f_exec = GOOD_TOOLS_STEP.replace(
        "[ -f \"$T\" ] || { echo \"ERROR: autodrive-tools-not-found: ${T}\" >&2; exit 1; }\n",
        "",
    );
    let (_, bad) = unchecked_tool_uses("no-f-exec", &no_f_exec);
    assert_eq!(
        bad.len(),
        1,
        "an executed tool with no -f check must be refused: {bad:?}"
    );
    // A check after the use does not count.
    let late = "TOOLS_D=\"${RECIPE_VAR_autodrive_tools_dir:-}\"\nbash \"${TOOLS_D}/x.sh\"\n[ -f \"${TOOLS_D}/x.sh\" ] || exit 1\n";
    assert_eq!(
        unchecked_tool_uses("late", late).1.len(),
        1,
        "a -f check after the use must be refused"
    );
}

/// The runner replaces an inherited `RECIPE_VAR_<key>` only for keys the
/// recipe declares (S2). An undeclared key, a read before its producer, or a
/// read after a producer that a `condition:` may skip, is the parent's value.
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
        "{} RECIPE_VAR_ reads the recipe does not declare (#1561 S2). Declare each key in the \
         recipe's `context:` with a \"\" default; see \"Every key read is declared in \
         `context:`\" in {DOCS_PAGE}:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_declared_key_check_flags_each_unsafe_read() {
    let tools = vec![(
        "t.sh".to_string(),
        "X=\"${RECIPE_VAR_tool_key:-}\"\n# ${RECIPE_VAR_commented_out:-}\n".to_string(),
    )];
    let yaml = r#"
name: fixture
context:
  declared: ""
steps:
  - id: "a-reads-early"
    type: bash
    command: |
      echo "${RECIPE_VAR_later:-}" "${RECIPE_VAR_declared:-}" "${RECIPE_VAR_nobody:-}"
  - id: "b-produces"
    type: bash
    command: echo '{}'
    output: later
  - id: "c-conditional"
    type: bash
    condition: "x == 'y'"
    command: echo '{}'
    output: maybe
  - id: "d-reads"
    type: bash
    command: |
      echo "${RECIPE_VAR_later__field:-}" "${RECIPE_VAR_maybe:-}"
      bash "$TOOLS/t.sh"
"#;
    let got = undeclared_reads(yaml, &tools);
    let has = |id: &str, key: &str, kind: &str| {
        got.iter()
            .any(|(i, k, w)| i == id && k == key && *w == kind)
    };
    assert!(has("a-reads-early", "nobody", "undeclared"), "{got:?}");
    assert!(
        has("a-reads-early", "later", "read-before-producer"),
        "{got:?}"
    );
    assert!(
        has("d-reads", "maybe", "read-after-conditional-producer"),
        "{got:?}"
    );
    assert!(
        has("d-reads", "tool_key", "undeclared"),
        "a tool's reads count: {got:?}"
    );
    assert!(!got.iter().any(|(_, k, _)| k == "declared"), "{got:?}");
    assert!(
        !got.iter()
            .any(|(i, k, _)| i == "d-reads" && k == "later__field"),
        "an unconditional earlier producer is safe: {got:?}"
    );
    assert!(!got.iter().any(|(_, k, _)| k == "commented_out"), "{got:?}");
    assert_eq!(got.len(), 4, "{got:?}");
}

/// `AUTODRIVE_TEST_KEEP_INHERITED` is a test-only switch: nothing the bundle
/// ships, and nothing the CLI runs, may look at it.
#[test]
fn the_keep_inherited_opt_out_is_test_only() {
    let root = repo_root();
    let mut stack = vec![root.join("amplifier-bundle"), root.join("crates")];
    let mut leaks = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(&root).unwrap().to_path_buf();
            let rel_s = rel.display().to_string();
            if rel_s.starts_with("amplifier-bundle/recipes/tests")
                || rel.components().any(|c| c.as_os_str() == "tests")
                || rel.components().any(|c| c.as_os_str() == "target")
            {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(text) = std::fs::read_to_string(&p)
                && text.contains(KEEP_INHERITED)
            {
                leaks.push(rel_s);
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "{KEEP_INHERITED} may appear only under amplifier-bundle/recipes/tests/ and tests/; \
         found in:\n{}",
        leaks.join("\n")
    );

    let harness = root.join("amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh");
    let text = std::fs::read_to_string(&harness).expect("read the contract shell test");
    assert!(
        text.contains(KEEP_INHERITED),
        "test-auto-drive-to-merge.sh must honour {KEEP_INHERITED}=1 so the suite can run \
         with the caller's AUTODRIVE_* variables left in place (#1607)"
    );
}

// ── Self-tests: a detector that stops matching fails here ───────────────────

#[test]
fn the_detectors_flag_every_spelling_of_the_bug() {
    for (text, detector) in [
        (r#"X="${FOO:-${RECIPE_VAR_foo:-}}""#, "nested-default"),
        // M4: an underscore in the bare name must not hide it.
        (r#"X="${FOO_BAR:-${RECIPE_VAR_x:-}}""#, "nested-default"),
        (
            r#"X="${FOO:-${RECIPE_VAR_foo:-MISSING}}""#,
            "nested-default",
        ),
        (r#"X=${FOO:-$RECIPE_VAR_foo}"#, "nested-default"),
        (r#"X="${FOO-${RECIPE_VAR_foo:-}}""#, "nested-default"),
        (
            r#": "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}}""#,
            "nested-default",
        ),
        (
            r#"PR_URL="${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}""#,
            "nested-default",
        ),
        (
            r#"for C in "${PR_NUMBER:-}" "${RECIPE_VAR_pr_number:-}" "$(g "$MG" pr '')"; do"#,
            "candidate-loop",
        ),
        (
            r#"for C in "${RECIPE_VAR_pr_number:-}" "${PR_NUMBER:-}"; do"#,
            "candidate-loop",
        ),
        (
            "X=\"${FOO:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"\n[ -n \"$X\" ] || X=\"{{foo}}\"",
            "two-line-fallback",
        ),
        (
            "WORKTREE_INPUT=\"${WORKTREE_SETUP_WORKTREE_PATH:-}\"\n\
             [ -n \"$WORKTREE_INPUT\" ] || WORKTREE_INPUT=\"${RECIPE_VAR_worktree_setup__worktree_path:-}\"",
            "two-line-fallback",
        ),
        (
            "X=\"${FOO:-}\"\nY=1\n[[ -n \"${X}\" ]] || X=\"${RECIPE_VAR_foo:-}\"",
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
        (
            r#"X="${FOO:-}"; [ -n "$X" ] || X="${RECIPE_VAR_foo:-}""#,
            "single-line-or",
        ),
        (r#"X="${RECIPE_VAR_foo:-${FOO:-}}""#, "bare-last"),
        (
            r#"W="${RECIPE_VAR_worktree_setup__worktree_path:-${WORKTREE_SETUP_WORKTREE_PATH:-}}""#,
            "bare-last",
        ),
        (r#"X="${RECIPE_VAR_foo:-$FOO}""#, "bare-last"),
        // A bare name after RECIPE_VAR_, whatever its name (rule 3.2/3.3).
        (
            r#"X="${RECIPE_VAR_foo:-${FOO_DEFAULT:-}}""#,
            "bare-in-chain",
        ),
        (
            r#"WORKTREE_SETUP_WORKTREE_PATH="${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}""#,
            "bare-in-chain",
        ),
        (
            r#": "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}}""#,
            "bare-in-chain",
        ),
        // The 4-level interleaved chain of workflow_final_status.sh:462.
        (
            r#"t="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${PUBLISH_STATE:-FOLLOWUP_CREATED}}}}""#,
            "nested-default",
        ),
        (
            r#"t="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${PUBLISH_STATE:-FOLLOWUP_CREATED}}}}""#,
            "bare-in-chain",
        ),
        (
            r#"U="${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}""#,
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
        r#"WORKTREE_SETUP_WORKTREE_PATH="${RECIPE_VAR_worktree_setup__worktree_path:-${RECIPE_VAR_repo_path:-}}""#,
        r#"U="${RECIPE_VAR_pr_url:-${RECIPE_VAR_pr_publish_result__pr_url:-}}""#,
        // M4: a RECIPE_VAR_ default is not a bare name, whatever its key.
        r#"X="${RECIPE_VAR_x:-${RECIPE_VAR_y:-}}""#,
        r#"X="${RECIPE_VAR_x:-${RECIPE_VAR_y:-${RECIPE_VAR_z:-d}}}""#,
        r#"X="${RECIPE_VAR_x-${RECIPE_VAR_y-}}""#,
        r#": "${TMPDIR:=/tmp}""#,
        r#"REPO="${REPO_PATH:-}""#,
        // A tool's own local, assigned earlier in the same script (D3).
        "PUBLISH_STATE=\"${RECIPE_VAR_pr_publish_result__state:-}\"\n\
         t=\"${RECIPE_VAR_terminal_state__terminal_state:-${PUBLISH_STATE:-FOLLOWUP_CREATED}}\"",
        "local FOO_DEFAULT=1\nX=\"${RECIPE_VAR_foo:-${FOO_DEFAULT:-}}\"",
        GOOD_OVERRIDE,
        r#"for C in "${RECIPE_VAR_pr_number:-}" "$(g "$MG" pr '')"; do"#,
        r#"for D in "${AMPLIHACK_HOME:-}" "${HOME:-}"; do"#,
        r#"[ -n "${CI:-}" ] || echo "${RECIPE_VAR_ci_evidence:-}""#,
        "# was ${FOO:-${RECIPE_VAR_foo:-}}",
        "    # X=\"${FOO:-}\" || X=\"${RECIPE_VAR_foo:-}\"",
        // A lone bare read that no RECIPE_VAR_ fallback follows.
        "X=\"${FOO:-}\"\necho \"$X\"",
        // The fallback is for a different local.
        "X=\"${FOO:-}\"\n[ -n \"$Y\" ] || Y=\"${RECIPE_VAR_foo:-}\"",
        // X was reassigned before the fallback, so the bare value is gone.
        "X=\"${FOO:-}\"\nX=\"$(cat f)\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"",
        // The fallback pairs with a different name.
        "X=\"${REPO_PATH:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_worktree_path:-}\"",
        "[ -n \"$X\" ] || X=\"{{foo}}\"",
    ] {
        let found = scan(text);
        assert!(
            found.is_empty(),
            "`{text}` must not be flagged; got {found:?}"
        );
    }
}

#[test]
fn a_violation_reports_path_line_detector_and_source_text() {
    let found = scan("ok=1\n  X=\"${FOO:-${RECIPE_VAR_foo:-}}\"\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].render("amplifier-bundle/recipes/x.yaml"),
        "amplifier-bundle/recipes/x.yaml:2: nested-default: X=\"${FOO:-${RECIPE_VAR_foo:-}}\""
    );
    // A two-line fallback is reported where the bare read is.
    let found = scan("a=1\nX=\"${FOO:-}\"\nb=2\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].line, 2);
    assert_eq!(found[0].detector, "two-line-fallback");
}

#[test]
fn names_pair_across_the_nested_field_separator() {
    assert!(pairs("FOO", "foo"));
    assert!(pairs(
        "WORKTREE_SETUP_WORKTREE_PATH",
        "worktree_setup__worktree_path"
    ));
    assert!(pairs("CI_EVIDENCE", "ci_evidence"));
    assert!(!pairs("QA_EVIDENCE", "qa_evidence_hash"));
    assert!(!pairs("QA_EVIDENCE_HASH", "qa_evidence"));
    assert!(!pairs("REPO_PATH", "worktree_setup__worktree_path"));
}

// ── Step 7 (TDD): the remaining contract for #1561 ───────────────────────────
//
// These tests pin the design decisions that the first guard did not yet cover:
// the cascade spelling (`if [ -z ]; then X=RECIPE_VAR_…; fi`), template
// self-comparisons, the per-site NO_MERGE directive and INFO line, the
// byte-identical normaliser, the probe's RECIPE_VAR_-only inputs, and the
// literal acceptance grep over the whole bundle.

/// The INFO line each NO_MERGE override site prints, and the variable it sets.
/// Mirrors the table in `docs/reference/auto-drive-to-merge.md#no_merge-override`.
const NO_MERGE_SITE_CONTRACT: &[(&str, &str, &str, &str)] = &[
    (
        "amplifier-bundle/recipes/autodrive-build.yaml",
        "step-01b-no-merge-directive",
        "NM",
        "INFO: autodrive-build/step-01b-no-merge-directive: no_merge=true from bare NO_MERGE env (override, #1561)",
    ),
    (
        "amplifier-bundle/recipes/autodrive-build.yaml",
        "step-03-resolve-pr",
        "NM",
        "INFO: autodrive-build/step-03-resolve-pr: no_merge=true from bare NO_MERGE env (override, #1561)",
    ),
    (
        "amplifier-bundle/recipes/workflow-terminal-state.yaml",
        "probe-terminal-state",
        "NO_MERGE_DIRECTIVE",
        "INFO: workflow-terminal-state/probe-terminal-state: no_merge=true from bare NO_MERGE env (override, #1561)",
    ),
];

const TERMINAL_RECIPE: &str = "amplifier-bundle/recipes/workflow-terminal-state.yaml";
const PROBE_STEP: &str = "probe-terminal-state";

/// `(local, RECIPE_VAR_ key, bare names it must no longer read)` for every
/// input of `probe-terminal-state`.
const PROBE_INPUTS: &[(&str, &str, &[&str])] = &[
    (
        "WORKTREE_INPUT",
        "worktree_setup__worktree_path",
        &["WORKTREE_SETUP_WORKTREE_PATH"],
    ),
    ("REPO_DIR", "repo_path", &["REPO_PATH"]),
    ("BRANCH_INPUT", "branch_name", &["BRANCH_NAME"]),
    ("BASE_INPUT", "base_ref", &["BASE_REF"]),
    ("PR_NUMBER_INPUT", "pr_number", &["PR_NUMBER"]),
    ("PR_URL_INPUT", "pr_url", &["PR_URL"]),
    (
        "GOAL_ALREADY_MET_INPUT",
        "goal_already_met",
        &["GOAL_ALREADY_MET"],
    ),
    ("fail_mode", "terminal_fail_mode", &["TERMINAL_FAIL_MODE"]),
];

fn read_repo_file(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn site_body(file: &str, id: &str) -> String {
    let yaml = read_repo_file(file);
    step_body(&yaml, id).unwrap_or_else(|| panic!("{file} has no step {id}"))
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Strips the common indentation so normalisers in steps nested at different
/// depths compare equal when, and only when, their text is the same.
fn dedent(text: &str) -> String {
    let indent = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    text.lines()
        .map(|l| {
            if l.len() >= indent {
                &l[indent..]
            } else {
                l.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Cascade detector (S3) ────────────────────────────────────────────────────

/// `(line, bare)` of every cascade: `X="${UPPER…}"` (or `X=$UPPER`) where UPPER
/// has a `RECIPE_VAR_<lower>` partner, followed within three non-comment lines
/// by any assignment of X from `RECIPE_VAR_<lower>` (after `||`, `&&`, `then`,
/// or on its own line inside an `if`). Whatever the connective, the bare name
/// was read first and wins whenever it is non-empty.
fn cascades(text: &str) -> Vec<(usize, String)> {
    let bare_assign = Regex::new(
        r#"^\s*(?:local\s+|export\s+)?([A-Za-z_][A-Za-z0-9_]*)="?\$\{?([A-Z][A-Z0-9_]*)\b"#,
    )
    .unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if is_comment(line) {
            continue;
        }
        let Some(c) = bare_assign.captures(line) else {
            continue;
        };
        let (local, bare) = (c[1].to_string(), c[2].to_string());
        if line.contains("RECIPE_VAR_") {
            continue; // a nested read: the main guard's detectors own it
        }
        let later = Regex::new(&format!(
            r#"(?:^|[\s;&|]){}="?\$\{{?RECIPE_VAR_([A-Za-z0-9_]+)"#,
            regex::escape(&local)
        ))
        .unwrap();
        let mut seen = 0;
        for next in lines.iter().skip(i + 1) {
            if is_comment(next) || next.trim().is_empty() {
                continue;
            }
            seen += 1;
            if seen > 3 {
                break;
            }
            if let Some(m) = later.captures(next)
                && pairs(&bare, &m[1])
            {
                found.push((i + 1, bare.clone()));
                break;
            }
        }
    }
    found
}

#[test]
fn the_cascade_detector_flags_every_connective() {
    for (text, what) in [
        (
            "X=\"${FOO:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"\n",
            "`[ -n ] ||`",
        ),
        (
            "X=\"${FOO:-}\"\n[ -z \"$X\" ] && X=\"${RECIPE_VAR_foo:-}\"\n",
            "`[ -z ] &&`",
        ),
        (
            "X=\"${FOO:-}\"\nif [ -z \"$X\" ]; then\n  X=\"${RECIPE_VAR_foo:-}\"\nfi\n",
            "an if block",
        ),
        (
            "X=\"${FOO:-}\"\nif [ -z \"$X\" ]; then X=\"${RECIPE_VAR_foo:-}\"; fi\n",
            "a one-line if",
        ),
        (
            "X=$FOO\n# a comment does not count\n\n[ -n \"$X\" ] || X=$RECIPE_VAR_foo\n",
            "unbraced, with a comment and a blank line between",
        ),
        (
            "WORKTREE_INPUT=\"${WORKTREE_SETUP_WORKTREE_PATH:-}\"\n[ -n \"$WORKTREE_INPUT\" ] || WORKTREE_INPUT=\"${RECIPE_VAR_worktree_setup__worktree_path:-}\"\n",
            "the pre-#1561 terminal-state worktree read",
        ),
        (
            "NO_MERGE_FLAG_INPUT=\"${NO_MERGE:-}\"\n[ -n \"$NO_MERGE_FLAG_INPUT\" ] || NO_MERGE_FLAG_INPUT=\"${RECIPE_VAR_no_merge:-}\"\n",
            "the pre-#1561 terminal-state NO_MERGE cascade",
        ),
    ] {
        assert!(
            !cascades(text).is_empty(),
            "{what} must be flagged as a cascade:\n{text}"
        );
    }
}

#[test]
fn the_cascade_detector_accepts_the_fixed_and_unrelated_spellings() {
    for text in [
        // The fix: one read, from RECIPE_VAR_ only.
        "X=\"${RECIPE_VAR_foo:-}\"\n[ -n \"$X\" ] || X=default\n",
        // A bare name with no partner (AMPLIHACK_HOME, TMPDIR) is out of scope.
        "X=\"${AMPLIHACK_HOME:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"\n",
        // A RECIPE_VAR_-to-RECIPE_VAR_ chain stays.
        "X=\"${RECIPE_VAR_a:-}\"\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_b:-}\"\n",
        // Too far apart to be one decision.
        "X=\"${FOO:-}\"\na=1\nb=2\nc=3\n[ -n \"$X\" ] || X=\"${RECIPE_VAR_foo:-}\"\n",
        // The NO_MERGE override: a separate OR after the recipe value.
        GOOD_OVERRIDE,
    ] {
        assert!(
            cascades(text).is_empty(),
            "must not be flagged: {:?}\n{text}",
            cascades(text)
        );
    }
}

#[test]
fn no_step_cascades_from_a_bare_name_to_its_recipe_var() {
    let hits: Vec<String> = scanned_files()
        .iter()
        .flat_map(|(path, text)| {
            let lines: Vec<&str> = text.lines().collect();
            cascades(text)
                .into_iter()
                .map(|(n, bare)| format!("{path}:{n}: {bare}: {}", lines[n - 1].trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "{} step-output reads try the inherited bare name before RECIPE_VAR_ (#1561 S3):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

// ── Template self-comparison detector (S1/S2) ────────────────────────────────

/// Lines whose shell compares a value with `"{{key}}"`. `render_shell` turns
/// `{{key}}` into `"$RECIPE_VAR_key"` outside a heredoc, so the comparison is
/// with the recipe's own value and discards every recipe-supplied input.
fn self_comparisons(text: &str) -> Vec<(usize, String)> {
    let tmpl = r#""?\{\{[A-Za-z0-9_.]+\}\}"?"#;
    let case_pat = Regex::new(&format!(r#"\bcase\s+\S+\s+in\b[^)]*{tmpl}[^)]*\)"#)).unwrap();
    let test_cmp = Regex::new(&format!(
        r#"(?:\[\[?|\btest)\s[^\]]*(?:!=|==|=)\s*{tmpl}\s*\]?\]?"#
    ))
    .unwrap();
    text.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && (case_pat.is_match(l) || test_cmp.is_match(l)))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

#[test]
fn the_self_comparison_detector_flags_each_shape() {
    for text in [
        r#"case "$BRANCH_INPUT" in "{{branch_name}}"|"''") BRANCH_INPUT="" ;; esac"#,
        r#"case "$fail_mode" in "{{terminal_fail_mode}}"|"") fail_mode="exit" ;; esac"#,
        r#"case "$X" in {{x}}) X="" ;; esac"#,
        r#"[ "$REPO_DIR" != "{{repo_path}}" ] || REPO_DIR=".""#,
        r#"[ "$X" = "{{x}}" ] && X="""#,
        r#"[[ "$X" == "{{x}}" ]] && X="""#,
        r#"if test "$X" != "{{x}}"; then :; fi"#,
    ] {
        assert_eq!(
            self_comparisons(text).len(),
            1,
            "a comparison with a rendered template must be flagged: {text}"
        );
    }
}

#[test]
fn the_self_comparison_detector_accepts_plain_empty_checks() {
    for text in [
        r#"case "$BRANCH_INPUT" in ""|"''") BRANCH_INPUT="" ;; esac"#,
        r#"[ -n "$X" ] || X="{{x}}""#,
        r#"[ "$fail_mode" = "report" ] && exit 0"#,
        r#"# case "$X" in "{{x}}") is how it used to look"#,
        r#"echo "{{x}}""#,
    ] {
        assert!(
            self_comparisons(text).is_empty(),
            "must not be flagged: {text}"
        );
    }
}

#[test]
fn no_recipe_compares_a_value_with_its_own_template() {
    let hits: Vec<String> = recipe_files()
        .iter()
        .flat_map(|(path, text)| {
            self_comparisons(text)
                .into_iter()
                .map(|(n, l)| format!("{path}:{n}: {l}"))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "{} shell comparisons test a value against its own rendered template \
         (render_shell turns \"{{{{x}}}}\" into \"\"$RECIPE_VAR_x\"\", so every \
         recipe-supplied value matches and is discarded). Test for empty or the \
         literal '' instead:\n{}",
        hits.len(),
        hits.join("\n")
    );
}

// ── The NO_MERGE sites, one by one (C3) ──────────────────────────────────────

#[test]
fn every_no_merge_site_sets_its_directive_and_prints_its_info_line() {
    for (file, id, directive, info) in NO_MERGE_SITE_CONTRACT {
        let body = site_body(file, id);
        let lines: Vec<&str> = body.lines().collect();
        let reads = no_merge_reads(&body);
        assert_eq!(
            reads.len(),
            1,
            "{file} {id}: exactly one bare NO_MERGE read, the override"
        );
        let line = lines[reads[0]];
        let sets = Regex::new(&format!(
            r#"&&\s*\{{?\s*{}=("?)true("?)\s*[;}}]"#,
            regex::escape(directive)
        ))
        .unwrap();
        assert!(
            sets.is_match(line),
            "{file} {id}: the override must switch `{directive}` to true through `&&`: {}",
            line.trim()
        );
        let to = (reads[0] + 4).min(lines.len());
        assert!(
            lines[reads[0]..to].iter().any(|l| l.contains(info)),
            "{file} {id}: within three lines of the override, the INFO line must be exactly\n  {info}"
        );
        assert!(
            body.contains(&format!(
                "{directive}=\"$(normalise_no_merge_flag \"${{RECIPE_VAR_no_merge:-}}\")\""
            )),
            "{file} {id}: `{directive}` must start as the normalised RECIPE_VAR_no_merge, \
             read directly (no intermediate copy, no template fallback)"
        );
        assert!(
            !body.contains("{{no_merge}}"),
            "{file} {id}: the `{{{{no_merge}}}}` template fallback must be gone; it rendered to the \
             same RECIPE_VAR_no_merge value"
        );
        assert!(
            !body.contains("NO_MERGE_FLAG_INPUT"),
            "{file} {id}: the bare-first NO_MERGE_FLAG_INPUT cascade must be gone"
        );
    }
}

#[test]
fn no_merge_info_lines_print_no_inherited_value() {
    let raw = Regex::new(r"\$\{?NO_MERGE\b").unwrap();
    for (file, id, _, info) in NO_MERGE_SITE_CONTRACT {
        let body = site_body(file, id);
        let line = body
            .lines()
            .find(|l| l.contains(info))
            .unwrap_or_else(|| panic!("{file} {id}: no INFO line"));
        assert!(
            !raw.is_match(
                // The override line itself reads NO_MERGE once, inside the
                // normaliser call; the echo part must not.
                line.split("echo").nth(1).unwrap_or(line)
            ),
            "{file} {id}: the INFO line must not print the raw NO_MERGE value (S8): {}",
            line.trim()
        );
    }
}

#[test]
fn the_no_merge_directive_is_never_cleared_after_the_override() {
    for (file, id, directive, _) in NO_MERGE_SITE_CONTRACT {
        let body = site_body(file, id);
        let lines: Vec<&str> = body.lines().collect();
        let at = no_merge_reads(&body)
            .first()
            .copied()
            .unwrap_or_else(|| panic!("{file} {id}: no override"));
        let assign = Regex::new(&format!(
            r#"(?:^|[\s;&|{{]){}=("?)([^\s;"}}]*)"#,
            regex::escape(directive)
        ))
        .unwrap();
        for (i, l) in lines.iter().enumerate().skip(at + 1) {
            if is_comment(l) {
                continue;
            }
            for c in assign.captures_iter(l) {
                assert_eq!(
                    &c[2],
                    "true",
                    "{file} {id}:{}: `{directive}` may only be set to true after the \
                     override, or a stale value could re-enable merging: {}",
                    i + 1,
                    l.trim()
                );
            }
        }
    }
}

#[test]
fn the_three_no_merge_normalisers_are_byte_identical() {
    let mut first: Option<(String, String)> = None;
    for (file, id, _, _) in NO_MERGE_SITE_CONTRACT {
        let body = site_body(file, id);
        let func = dedent(
            &normaliser_in(&body)
                .unwrap_or_else(|| panic!("{file} {id} defines no normalise_no_merge_flag()")),
        );
        match &first {
            None => first = Some((format!("{file} {id}"), func)),
            Some((where0, f0)) => assert_eq!(
                &func, f0,
                "{file} {id}: normalise_no_merge_flag differs from the copy in {where0}; \
                 copy it byte for byte"
            ),
        }
    }
}

#[test]
fn every_site_defines_the_normaliser_before_reading_the_recipe_value() {
    for (file, id, _, _) in NO_MERGE_SITE_CONTRACT {
        let body = site_body(file, id);
        let lines: Vec<&str> = body.lines().collect();
        let def = lines
            .iter()
            .position(|l| l.trim_start().starts_with("normalise_no_merge_flag()"))
            .unwrap_or_else(|| panic!("{file} {id}: no normaliser"));
        let first_read = lines
            .iter()
            .position(|l| !is_comment(l) && l.contains("RECIPE_VAR_no_merge"))
            .unwrap_or_else(|| panic!("{file} {id}: never reads RECIPE_VAR_no_merge"));
        assert!(
            def < first_read,
            "{file} {id}: define normalise_no_merge_flag above the RECIPE_VAR_no_merge read"
        );
    }
    // In the probe the normaliser also moves above the input block.
    let body = site_body(TERMINAL_RECIPE, PROBE_STEP);
    let lines: Vec<&str> = body.lines().collect();
    let def = lines
        .iter()
        .position(|l| l.trim_start().starts_with("normalise_no_merge_flag()"))
        .expect("probe normaliser");
    let input = lines
        .iter()
        .position(|l| {
            !is_comment(l)
                && Regex::new(r"^\s*(WORKTREE_INPUT|REPO_DIR|BRANCH_INPUT)=")
                    .unwrap()
                    .is_match(l)
        })
        .expect("probe input block");
    assert!(
        def < input,
        "{TERMINAL_RECIPE} {PROBE_STEP}: the normaliser must be defined above the input reads"
    );
}

// ── The terminal-state probe reads RECIPE_VAR_ only (C1a) ───────────────────

#[test]
fn the_terminal_probe_reads_each_input_once_from_recipe_var() {
    let body = site_body(TERMINAL_RECIPE, PROBE_STEP);
    let code: Vec<&str> = body.lines().filter(|l| !is_comment(l)).collect();
    for (local, key, bares) in PROBE_INPUTS {
        let first = format!("{local}=\"${{RECIPE_VAR_{key}:-}}\"");
        let assigns: Vec<&&str> = code
            .iter()
            .filter(|l| {
                Regex::new(&format!(r"(?:^|[\s;&|]){}=", regex::escape(local)))
                    .unwrap()
                    .is_match(l)
            })
            .collect();
        assert!(
            assigns.iter().any(|l| l.trim() == first),
            "{PROBE_STEP}: `{local}` must be read as `{first}`; assignments now:\n{}",
            assigns
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n")
        );
        for l in &assigns {
            assert!(
                !l.contains(&format!("{{{{{key}}}}}")),
                "{PROBE_STEP}: `{local}` must not fall back to the `{{{{{key}}}}}` template, \
                 which renders to the same RECIPE_VAR_{key}: {}",
                l.trim()
            );
            for bare in *bares {
                assert!(
                    !Regex::new(&format!(r"\$\{{?{bare}\b")).unwrap().is_match(l),
                    "{PROBE_STEP}: `{local}` must not read the inherited bare `{bare}`: {}",
                    l.trim()
                );
            }
        }
    }
}

#[test]
fn the_terminal_probe_blanks_inputs_only_when_empty_or_quoted_empty() {
    let body = site_body(TERMINAL_RECIPE, PROBE_STEP);
    for local in [
        "BRANCH_INPUT",
        "BASE_INPUT",
        "PR_NUMBER_INPUT",
        "PR_URL_INPUT",
    ] {
        let blank = Regex::new(&format!(
            r#"case "\${local}" in ""\|"''"\) {local}="" ;; esac"#
        ))
        .unwrap();
        assert!(
            blank.is_match(&body),
            "{PROBE_STEP}: `{local}` must be blanked by `case \"${local}\" in \"\"|\"''\") {local}=\"\" ;; esac` \
             and by nothing that names its template"
        );
    }
    assert!(
        Regex::new(r#"case "\$GOAL_ALREADY_MET_INPUT" in ""\|"''"\) GOAL_ALREADY_MET_INPUT="false" ;; esac"#)
            .unwrap()
            .is_match(&body),
        "{PROBE_STEP}: an empty goal_already_met must default to false, and only an empty one"
    );
    assert!(
        !body.contains("{{repo_path}}"),
        "{PROBE_STEP}: `[ \"$REPO_DIR\" != \"{{{{repo_path}}}}\" ]` replaced every recipe repo_path with `.`"
    );
    assert!(
        !body.contains("{{terminal_fail_mode}}"),
        "{PROBE_STEP}: the `{{{{terminal_fail_mode}}}}` case reset `report` to `exit`"
    );
}

/// The probe step runs as `render_shell` would render it, with nothing but
/// RECIPE_VAR_ inputs and inherited bare decoys: the recipe values win.
#[test]
fn the_rendered_probe_input_block_uses_recipe_values_over_bare_decoys() {
    let body = site_body(TERMINAL_RECIPE, PROBE_STEP);
    // Run only the input-resolution prefix: up to `TARGET_REPO_DIR` selection.
    let lines: Vec<&str> = body.lines().collect();
    let end = lines
        .iter()
        .position(|l| l.trim_start().starts_with("[ -n \"$TARGET_REPO_DIR\" ]"))
        .expect("probe selects TARGET_REPO_DIR");
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("normalise_no_merge_flag()"))
        .expect("probe normaliser");
    let start = start.min(
        lines
            .iter()
            .position(|l| Regex::new(r"^\s*WORKTREE_INPUT=").unwrap().is_match(l))
            .expect("probe input block"),
    );
    let prefix = lines[start..end].join("\n");
    let rendered = Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}")
        .unwrap()
        .replace_all(&prefix, "\"$$RECIPE_VAR_$1\"")
        .to_string();
    let script = format!(
        "set -euo pipefail\n{rendered}\n\
         printf '%s|%s|%s|%s|%s|%s|%s|%s\\n' \"$TARGET_REPO_DIR\" \"$BRANCH_INPUT\" \"$BASE_INPUT\" \
         \"$PR_NUMBER_INPUT\" \"$PR_URL_INPUT\" \"$GOAL_ALREADY_MET_INPUT\" \"$NO_MERGE_DIRECTIVE\" \
         \"$(active_should_merge)\"\n"
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RECIPE_VAR_repo_path", "/recipe/repo")
        .env("RECIPE_VAR_branch_name", "feat/recipe-1561")
        .env("RECIPE_VAR_base_ref", "recipe-base")
        .env("RECIPE_VAR_pr_number", "4242")
        .env("RECIPE_VAR_pr_url", "https://github.com/o/r/pull/4242")
        .env("RECIPE_VAR_goal_already_met", "true")
        .env("RECIPE_VAR_no_merge", "true")
        .env("REPO_PATH", "/stale/repo")
        .env("WORKTREE_SETUP_WORKTREE_PATH", "/stale/worktree")
        .env("BRANCH_NAME", "stale-branch")
        .env("BASE_REF", "stale-base")
        .env("PR_NUMBER", "1")
        .env("PR_URL", "https://github.com/stale/stale/pull/1")
        .env("GOAL_ALREADY_MET", "false")
        .env("NO_MERGE", "false")
        .output()
        .expect("run bash");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "rendered input block failed: {}\n{}",
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "/recipe/repo|feat/recipe-1561|recipe-base|4242|https://github.com/o/r/pull/4242|true|true|false",
        "the recipe's own values must reach the probe, a bare NO_MERGE=false must not clear \
         a recipe no_merge=true, and no inherited bare value may be used"
    );
}

// ── Acceptance grep and docs (step 4) ────────────────────────────────────────

/// The literal acceptance criterion: no `${UPPER:-${RECIPE_VAR_` anywhere under
/// `amplifier-bundle/`, comments and test scripts included.
#[test]
fn the_literal_acceptance_grep_finds_nothing_in_the_bundle() {
    let re = Regex::new(r"\$\{[A-Z_][A-Z0-9_]*:-\$\{RECIPE_VAR_").unwrap();
    let mut hits = Vec::new();
    let mut stack = vec![repo_root().join("amplifier-bundle")];
    let root = repo_root();
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("read dir") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue; // binary
            };
            for (i, l) in text.lines().enumerate() {
                if re.is_match(l) {
                    hits.push(format!(
                        "{}:{}: {}",
                        p.strip_prefix(&root).unwrap().display(),
                        i + 1,
                        l.trim()
                    ));
                }
            }
        }
    }
    hits.sort();
    assert!(
        hits.is_empty(),
        "the acceptance grep still matches {} lines under amplifier-bundle/ (comments \
         count: describe the old order in words instead):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

#[test]
fn the_docs_describe_the_new_precedence_and_drop_the_old_anchor() {
    let page = read_repo_file(DOCS_PAGE);
    let anchors: BTreeSet<String> = page
        .lines()
        .filter_map(|l| l.strip_prefix('#'))
        .map(|l| anchor_of(l.trim_start_matches('#')))
        .collect();
    for want in [
        "step-outputs-ignore-inherited-bare-variables",
        NO_MERGE_ANCHOR,
        "template-fallbacks-compared-a-value-with-itself",
    ] {
        assert!(
            anchors.contains(want),
            "{DOCS_PAGE} has no heading with anchor #{want}"
        );
    }
    assert!(
        !anchors.contains("a-stale-bare-variable-wins"),
        "{DOCS_PAGE} still has the old \"A stale bare variable wins\" section"
    );
    for (_, _, _, info) in NO_MERGE_SITE_CONTRACT {
        assert!(
            page.contains(info),
            "{DOCS_PAGE} must list the INFO line `{info}`"
        );
    }
    // No page links to the removed anchor.
    let mut stack = vec![repo_root().join("docs")];
    let mut stale = Vec::new();
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("read docs") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md")
                && std::fs::read_to_string(&p)
                    .unwrap_or_default()
                    .contains("a-stale-bare-variable-wins")
            {
                stale.push(p.display().to_string());
            }
        }
    }
    assert!(
        stale.is_empty(),
        "pages still link to #a-stale-bare-variable-wins: {stale:?}"
    );
    // Every same-page link to the new anchors resolves.
    let link = Regex::new(r"\]\(#([A-Za-z0-9_-]+)\)").unwrap();
    let broken: Vec<String> = link
        .captures_iter(&page)
        .map(|c| c[1].to_string())
        .filter(|a| !anchors.contains(a))
        .collect();
    assert!(
        broken.is_empty(),
        "{DOCS_PAGE} links to anchors it does not have: {broken:?}"
    );
}
