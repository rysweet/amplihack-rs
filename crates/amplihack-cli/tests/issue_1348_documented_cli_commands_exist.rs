//! Issue #1348 — a documented command must exist.
//!
//! `docs/recipes/README.md` described `amplihack recipe manifest update`,
//! `amplihack recipe manifest check`, and `amplihack recipe sync`, with a
//! recommended workflow built around them. None of the three exist. The real
//! subcommands are `run`, `list`, `validate` and `show`.
//!
//! Documentation that names a command nobody can run is worse than no
//! documentation: it sends the reader looking for a fault in their own
//! environment. This guard reads the docs and fails when a fenced example
//! invokes a subcommand the CLI does not define.
//!
//! Scope is deliberately fenced code blocks only. A command named in prose may
//! be under discussion — this file's own README section says those three do not
//! exist, and that sentence must not trip the check. A command inside a fence is
//! an instruction to copy and run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Top-level subcommands the CLI defines, taken from the clap enum's source so
/// the list cannot drift from the binary without this file noticing.
fn defined_top_level(root: &Path) -> BTreeSet<String> {
    // The top-level enum lives in cli_commands.rs; `recipe`'s subcommands live
    // in cli_subcommands.rs. Read both rather than guessing.
    variants_of(
        &root.join("crates/amplihack-cli/src/cli_commands.rs"),
        "pub enum Commands",
    )
}

/// Variant names of a clap enum, kebab-cased the way clap renames them.
///
/// Deliberately shallow: it takes identifiers that begin a line inside the
/// enum's braces and stops at the closing brace. Nested braces from struct-style
/// variants are tracked so their fields are not mistaken for variants.
fn variants_of(path: &Path, header: &str) -> BTreeSet<String> {
    let src =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut out = BTreeSet::new();
    let Some(pos) = src.find(header) else {
        return out;
    };
    let mut depth = 0i32;
    for line in src[pos..].lines() {
        let before = depth;
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        let t = line.trim();
        if before == 1
            && t.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && !t.starts_with("///")
        {
            let name: String = t
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !name.is_empty() {
                out.insert(to_kebab(&name));
            }
        }
        if before > 0 && depth == 0 {
            break;
        }
    }
    out
}

fn to_kebab(variant: &str) -> String {
    let mut out = String::new();
    for (i, ch) in variant.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Subcommands of `amplihack recipe`, read from its own args struct.
fn defined_recipe_subcommands(root: &Path) -> BTreeSet<String> {
    variants_of(
        &root.join("crates/amplihack-cli/src/cli_subcommands.rs"),
        "pub enum RecipeCommands",
    )
}

/// Every `amplihack ...` invocation inside a fenced code block.
fn fenced_invocations(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (n, line) in body.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            continue;
        }
        let t = line.trim();
        // Skip comments and shell prompts' surrounding noise.
        let t = t.strip_prefix("$ ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("amplihack ") {
            out.push((n + 1, rest.to_string()));
        }
    }
    out
}

#[test]
fn every_amplihack_command_in_a_fenced_doc_example_exists() {
    let root = repo_root();
    let top = defined_top_level(&root);
    let recipe_subs = defined_recipe_subcommands(&root);
    assert!(
        top.contains("recipe"),
        "could not parse the CLI's own subcommand list; the guard would pass vacuously"
    );
    assert!(
        recipe_subs.contains("run") && recipe_subs.contains("list"),
        "could not parse `amplihack recipe`'s subcommands; got {recipe_subs:?}"
    );

    let doc = root.join("docs/recipes/README.md");
    let body = std::fs::read_to_string(&doc).expect("read docs/recipes/README.md");

    let mut findings = Vec::new();
    for (line, invocation) in fenced_invocations(&body) {
        let mut words = invocation.split_whitespace();
        let Some(first) = words.next() else { continue };
        if first.starts_with('-') {
            continue; // a bare flag, e.g. `amplihack --version`
        }
        if !top.contains(first) {
            findings.push(format!(
                "  docs/recipes/README.md:{line}  `amplihack {first}` is not a subcommand"
            ));
            continue;
        }
        if first == "recipe"
            && let Some(sub) = words.next()
            && !sub.starts_with('-')
            && !recipe_subs.contains(sub)
        {
            findings.push(format!(
                "  docs/recipes/README.md:{line}  `amplihack recipe {sub}` does not exist \
                 (have: {})",
                recipe_subs.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }

    assert!(
        findings.is_empty(),
        "documentation invokes commands that do not exist:\n{}\n\n\
         A reader who copies one of these will look for a fault in their own \
         environment. Either implement the command or correct the document \
         (issue #1348).",
        findings.join("\n")
    );
}
