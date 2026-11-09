//! Issue #2181: bundled agent and skill text must not tell agents never to
//! merge, and must not make a merge wait for a person's approval.
//!
//! An agent stops at a mergeable PR. The caller's workflow decides whether to
//! merge: either the auto-drive-to-merge gate, or a caller that passes
//! `no_merge` or `autodrive_dry_run`. Blanket wording such as "never
//! auto-merge" or "wait for an explicit merge request from the user" overrode
//! that gate, because every agent that loaded the text treated it as a rule.
//!
//! `bundle_has_no_blanket_no_merge_text` scans every file under
//! `amplifier-bundle/agents`, `amplifier-bundle/skills` and
//! `amplifier-bundle/recipes`, plus `amplifier-bundle/bundle.md`, and reports
//! each offending line as `path:line: text (reason)`.
//!
//! `matcher_self_check` runs the matcher over fixed samples, so a weakened
//! pattern fails even when the tree is clean.
//!
//! Conditional no-merge rules (an explicit `no_merge` flag, the gate's own
//! "never merges" and "never MERGE_READY" wording, "external approval" in
//! `merge-ready/SKILL.md`, `approval_status`, production-deploy approvals) are
//! not flagged. If a legitimate line is flagged, narrow the pattern; do not
//! grow the allowlist without a linked issue, and never exclude a file.
//!
//! The scan only reads files: no subprocesses, network calls, sleeps,
//! timeouts or retries.

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/amplihack-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Lowercase substrings. A line containing any of them is reported whatever
/// else it says.
const BANNED_SUBSTRINGS: &[(&str, &str)] = &[
    (
        "never auto-merge",
        "blanket no-merge rule; the caller's workflow decides the merge",
    ),
    (
        "do not merge automatically",
        "blanket no-merge rule; the caller's workflow decides the merge",
    ),
    (
        "auto-merge without permission",
        "ties the merge to a person's permission",
    ),
    (
        "human approval always required",
        "ties the merge to a person's approval",
    ),
    (
        "not auto-merged",
        "blanket no-merge rule; the caller's workflow decides the merge",
    ),
    (
        "merge (but don't)",
        "blanket no-merge rule; the caller's workflow decides the merge",
    ),
];

/// Applied only to lines whose lowercase form contains `merge`. Both accept a
/// straight or curly apostrophe and have word boundaries, so `superuser` and
/// `username` do not match.
const APPROVAL_PATTERNS: &[(&str, &str)] = &[
    (
        r"\b(human|owner|user|your)(['\x{2019}]s)? (explicit )?(approval|permission|merge request|merge command|request)\b",
        "ties the merge to a person's approval",
    ),
    (
        r"\b(approval|permission|request|command) (from|of|by) (the |a )?(human|owner|user)\b",
        "ties the merge to a person's approval",
    ),
];

/// Exact trimmed lines that are allowed, each with its reason. Never a whole
/// file or directory. A new entry needs a linked issue.
const ALLOWED_LINES: &[(&str, &str)] = &[(
    "# workflow never auto-merges when told not to. The intent is a",
    "issue #929: applies only when an explicit no_merge flag is set \
     (recipes/workflow-terminal-state.yaml)",
)];

struct Matcher {
    patterns: Vec<(Regex, &'static str)>,
}

impl Matcher {
    fn new() -> Self {
        let patterns = APPROVAL_PATTERNS
            .iter()
            .map(|(p, why)| (Regex::new(p).expect("approval pattern compiles"), *why))
            .collect();
        Self { patterns }
    }

    /// Returns the reason a line violates the rule, or `None` if it does not.
    fn violation(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if ALLOWED_LINES.iter().any(|(allowed, _)| *allowed == trimmed) {
            return None;
        }
        let lower = trimmed.to_lowercase();
        for (needle, why) in BANNED_SUBSTRINGS {
            if lower.contains(needle) {
                return Some(format!("banned phrase {needle:?}: {why}"));
            }
        }
        if lower.contains("merge") {
            for (re, why) in &self.patterns {
                if let Some(m) = re.find(&lower) {
                    return Some(format!("approval wording {:?}: {why}", m.as_str()));
                }
            }
        }
        None
    }
}

/// Recursive walk with sorted entries. Symlinks are skipped through
/// `symlink_metadata`; their targets are inside the scanned tree and are
/// walked directly. No extension filter.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
                .path()
        })
        .collect();
    entries.sort();
    for path in entries {
        let meta = fs::symlink_metadata(&path)
            .unwrap_or_else(|e| panic!("symlink_metadata {}: {e}", path.display()));
        let ft = meta.file_type();
        if ft.is_symlink() {
            continue;
        } else if ft.is_dir() {
            collect_files(&path, out);
        } else if ft.is_file() {
            out.push(path);
        }
    }
}

#[test]
fn bundle_has_no_blanket_no_merge_text() {
    let root = repo_root();
    let bundle = root.join("amplifier-bundle");

    let mut files = Vec::new();
    for sub in ["agents", "skills", "recipes"] {
        let dir = bundle.join(sub);
        assert!(
            dir.is_dir(),
            "scan root {} is missing; the check must not pass on an empty tree",
            dir.display()
        );
        collect_files(&dir, &mut files);
    }
    let catalogue = bundle.join("bundle.md");
    assert!(
        catalogue.is_file(),
        "{} is missing; the agent catalogue must be scanned",
        catalogue.display()
    );
    files.push(catalogue);
    assert!(!files.is_empty(), "no bundle files were scanned");

    let matcher = Matcher::new();
    let mut violations = Vec::new();
    for path in &files {
        let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let text = String::from_utf8_lossy(&bytes);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        for (idx, line) in text.lines().enumerate() {
            if let Some(why) = matcher.violation(line) {
                violations.push(format!("  {rel}:{}: {} ({why})", idx + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "bundled text tells agents never to merge or ties a merge to a person's approval \
         (issue #2181):\n{}\n\nReword each line so the agent stops at mergeable and the \
         caller's workflow decides the merge.",
        violations.join("\n")
    );
}

#[test]
fn matcher_self_check() {
    let matcher = Matcher::new();

    let must_hit = [
        // One sample per banned substring, some in upper or mixed case.
        "- **Never Auto-Merge**: Stop at mergeable state",
        "Do NOT merge automatically - wait for:",
        "- NEVER auto-merge without permission",
        "- Never auto-merge: human approval always required",
        "Human Approval Always Required before merge",
        "PR is ready but NOT auto-merged.",
        "7. **MERGEABLE**: Ready to merge (but DON'T)",
        // Approval patterns.
        "2. Explicit merge request from user",
        "2. Your explicit merge command",
        "The goal: Transform \"CI is failing\" into \"PR ready to merge, awaiting your approval\"",
        "merge requires the owner's approval",
        "merge requires the owner\u{2019}s approval",
        "Merge only with permission from the human",
        "- **To merger**: Only after approval by the user",
    ];
    for sample in must_hit {
        assert!(
            matcher.violation(sample).is_some(),
            "matcher must flag: {sample:?}"
        );
    }

    let must_not_hit = [
        // The allowlisted #929 line, as it appears indented in the recipe.
        "      # workflow never auto-merges when told not to. The intent is a",
        // Gate wording and conditional rules.
        "Phase 1 never merges",
        "never MERGE_READY until the gate passes",
        "Don't merge similar concepts",
        "If any criterion is missing, stale, inaccessible, or blocked by external approval, \
         check, or policy state, the PR/pull request is **not merge-ready**.",
        "| 6. Reviews/approvals | A bash step (`autodrive_platform_facts.sh`) measures \
         `approval_status`. |",
        "refused IV2-placeholders \"$L_API\" api -X PUT 'repos/{owner}/{repo}/pulls/38/merge'",
        "Production deploy requires approval from the release manager",
        // Word boundaries.
        "Merge the superuser request handler first",
        "gh search prs \"author:username is:merged\"",
        // Replacement wording for #2181.
        "description: CI failure resolution workflow. Monitors CI status after push, \
         diagnoses failures, fixes issues, and iterates until the PR is mergeable, then \
         stops; the caller's workflow (the auto-drive-to-merge gate, or a caller passing \
         no_merge / autodrive_dry_run) decides the merge. Use when CI checks fail after \
         pushing code.",
        "- **Stop at Mergeable**: Once the PR is mergeable, stop; the caller's workflow owns the merge",
        "CI is green. When the required reviews are in, the PR is mergeable; the agent stops \
         there and the caller's workflow decides the merge.",
        "PASSED → MERGEABLE → HANDED_TO_CALLER",
        "7. **MERGEABLE**: Ready to merge; the agent stops here",
        "8. **HANDED_TO_CALLER**: Success; the caller's workflow decides the merge",
        "- **To merger**: Through the caller's workflow (auto-drive-to-merge gate)",
        "- Stop at mergeable; never merge outside the caller's workflow",
        "The goal: Transform \"CI is failing\" into \"PR mergeable and handed to the caller's \
         workflow\" through intelligent automation.",
        "- Stops at a mergeable PR: the caller's workflow (auto-drive-to-merge gate, or \
         no_merge / autodrive_dry_run) decides the merge",
        "- Stops at mergeable state; the caller's workflow decides the merge",
    ];
    for sample in must_not_hit {
        assert_eq!(
            matcher.violation(sample),
            None,
            "matcher must not flag: {sample:?}"
        );
    }
}
