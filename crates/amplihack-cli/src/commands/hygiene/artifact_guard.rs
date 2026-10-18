use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use amplihack_utils::artifact_guard::{
    self, ArtifactGuardConfig, ArtifactGuardMode, ArtifactGuardReport, ArtifactSource,
    ArtifactViolation, GENERIC_OUTPUT_SEGMENTS, PreExistingPolicy, scan_artifacts,
};
use anyhow::Result;

use crate::{HygieneArtifactGuardArgs, command_error};

/// Environment overrides for the two provenance controls (issue #1422). The CLI
/// flags win; these exist so a workflow step that cannot edit its recipe's guard
/// invocation can still set the policy.
const BASELINE_ENV: &str = "AMPLIHACK_ARTIFACT_GUARD_BASELINE";
const PREEXISTING_ENV: &str = "AMPLIHACK_ARTIFACT_GUARD_PREEXISTING";

/// Individual paths listed per artifact root before the report summarises the
/// rest. Pre-existing conditions run to hundreds of paths (issue #1422 hit 668);
/// naming every one of them buries the remedy that follows.
const ADVISORY_PATHS_PER_ROOT: usize = 5;

pub fn run(args: HygieneArtifactGuardArgs) -> Result<()> {
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir().map_err(|error| {
            eprintln!("Artifact Guard configuration error: resolve current directory: {error}");
            command_error::exit_error(2)
        })?,
    };

    let mode = match ArtifactGuardMode::parse(&args.mode) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("Artifact Guard configuration error: {error}");
            return Err(command_error::exit_error(2));
        }
    };

    let policy_input = args
        .preexisting
        .clone()
        .or_else(|| std::env::var(PREEXISTING_ENV).ok())
        .filter(|value| !value.trim().is_empty());
    let policy = match policy_input.as_deref().map(PreExistingPolicy::parse) {
        Some(Ok(policy)) => policy,
        Some(Err(error)) => {
            eprintln!("Artifact Guard configuration error: {error}");
            return Err(command_error::exit_error(2));
        }
        None => PreExistingPolicy::default(),
    };

    let mut config = ArtifactGuardConfig::new(&repo)
        .with_mode(mode)
        .with_preexisting_policy(policy);
    if let Some(baseline) = args
        .baseline
        .clone()
        .or_else(|| std::env::var(BASELINE_ENV).ok())
        .filter(|value| !value.trim().is_empty())
    {
        config = config.with_baseline(baseline);
    }
    if let Some(allowlist) = args.allowlist {
        config = config.with_allowlist(allowlist);
    } else {
        let default_allowlist = PathBuf::from(".amplihack-artifact-allowlist");
        if repo.join(&default_allowlist).exists() {
            config = config.with_allowlist(default_allowlist);
        }
    }

    match scan_artifacts(&config) {
        Ok(report) => {
            // Pre-existing findings are printed whether or not they block, so a
            // gate that passes still tells the operator the repository is dirty.
            print_advisory_report(&report);
            if report.blocks() {
                print_violation_report(&report);
                return Err(command_error::exit_error(1));
            }
            println!(
                "Artifact Guard clean: no prohibited artifacts introduced by this change in {} (mode: {}).",
                report.repo.display(),
                report.mode
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("Artifact Guard configuration error: {error}");
            eprintln!(
                "Fix the repository path, Git state, mode, allowlist, or baseline and rerun the guard."
            );
            Err(command_error::exit_error(2))
        }
    }
}

fn print_violation_report(report: &ArtifactGuardReport) {
    let blocking = report.blocking_violations();
    let scope = if report.preexisting_is_advisory() {
        " introduced by this change"
    } else {
        ""
    };
    eprintln!(
        "Artifact Guard blocked {} prohibited artifact path(s){scope} in {} (mode: {}).",
        blocking.len(),
        report.repo.display(),
        report.mode
    );
    if let Some(baseline) = &report.baseline {
        eprintln!(
            "Baseline: {} ({}).",
            baseline.rev,
            short_commit(&baseline.commit)
        );
    }
    eprintln!();
    eprintln!("{:<16} {:<14} {:<48} rule", "source", "provenance", "path");
    for violation in &blocking {
        // `{:<width}` cannot pad a Display impl that writes straight to the
        // stream, so widen the rendered strings instead and keep the table
        // readable.
        eprintln!(
            "{:<16} {:<14} {:<48} {}",
            violation.source.to_string(),
            violation.provenance.to_string(),
            display_path(&violation.path),
            violation.rule_id
        );
    }
    eprintln!();
    eprintln!("Remediation:");
    eprintln!("  - Remove local artifact leftovers from the parent worktree.");
    eprintln!(
        "  - Move generated, plugin, cache, and runtime output into an ignored isolated directory outside the parent worktree."
    );
    eprintln!(
        "  - If intentional source material, add a narrow reviewed entry to .amplihack-artifact-allowlist."
    );
    eprintln!();
    eprintln!("Exact commands for the paths above:");
    // Collapsing to an artifact root is only safe when the change owns the
    // whole root. If a pre-existing tree shares it -- the issue #1422 shape,
    // where one new package lands inside 668 vendored ones -- a
    // `git rm -r --cached node_modules` would untrack all 668 as a side effect,
    // which is precisely the unrelated change the run was told not to make.
    //
    // Removal commands come only from `removal_root`, never from the display
    // grouping, and never for a path that may be source (issue #1536).
    let (removable, review) = split_removable(&blocking);
    let advisory = report.advisory_violations();
    let untouchable: Vec<&str> = advisory
        .iter()
        .chain(review.iter())
        .map(|violation| violation.path.as_str())
        .collect();
    for (root, group) in removable {
        if untouchable.iter().any(|path| is_within(path, &root)) {
            for violation in &group {
                let path = &violation.path;
                // The root is command-safe, but the rest of the path is not
                // checked by `split_removable`.
                if !is_command_safe(path) {
                    eprintln!(
                        "  # {path:?} has a name that is unsafe in a shell command; handle it by hand after review"
                    );
                } else if is_indexed(violation) {
                    eprintln!(
                        "  {}   # drop from the change, keep on disk",
                        git_rm_cached(path, false)
                    );
                } else {
                    // Not in the index: `git rm --cached` would fail with
                    // "pathspec did not match any files".
                    eprintln!("  rm -- '{path}'   # delete the untracked leftover");
                }
            }
            eprintln!(
                "  # (listed per path: '{root}' also holds pre-existing entries this change must not touch)"
            );
        } else {
            if group.iter().any(|violation| is_indexed(violation)) {
                eprintln!(
                    "  {}   # drop from the change, keep on disk",
                    git_rm_cached(&root, true)
                );
            }
            eprintln!("  rm -rf -- '{root}'               # or delete the leftover outright");
        }
    }
    if !review.is_empty() {
        eprintln!(
            "  Possibly source (matches a generic output directory name, or its name is unsafe in a shell command); the guard will not suggest removing it:"
        );
        for (root, group) in group_by_root(&review) {
            print_path_group(&root, &group);
        }
        print_review_remedy(&review);
    }
    eprintln!(
        "  amplihack hygiene artifact-guard --repo {} --mode {}   # re-run to confirm",
        report.repo.display(),
        report.mode
    );
    eprintln!();
    eprintln!("First violation detail:");
    if let Some(first) = blocking.first() {
        // The rule's generic remediation opens with "remove the artifact".
        // For a path the guard has just called possibly source, the last line
        // an agent reads must not tell it to delete that path (issue #1536).
        let detail = if is_removable(first) {
            first.remediation.as_str()
        } else {
            REVIEW_DETAIL
        };
        eprintln!("  {}: {}", display_path(&first.path), detail);
    }
}

/// The first-violation detail for a path with no safe removal root: it may be
/// source, so it is pointed at review and the allowlist, never at deletion.
const REVIEW_DETAIL: &str = "possibly source, so the guard does not suggest removing it; review the path, and if it is intentional add a narrow reviewed .amplihack-artifact-allowlist entry, otherwise rename it";

/// Whether the guard may print a removal command for `violation`: it has a
/// removal root and that root is safe to quote in a shell command.
fn is_removable(violation: &ArtifactViolation) -> bool {
    matches!(violation.removal_root.as_deref(), Some(root) if is_command_safe(root))
}

/// Report the pre-existing condition without failing the gate.
///
/// Issue #1422: a run that had already designed, implemented, tested, and
/// verified its work was killed at its post-implementation checkpoint because
/// the repository it was working in had 668 `node_modules` paths committed
/// years earlier. The guard's finding was true and none of its three suggested
/// remedies fit inside that change's scope. A pre-existing tracked artifact is
/// already on the baseline, so no gate can keep it out of anything; blocking
/// only discards the verified work standing in front of it.
fn print_advisory_report(report: &ArtifactGuardReport) {
    let advisory = report.advisory_violations();
    if advisory.is_empty() {
        return;
    }
    eprintln!(
        "Artifact Guard: {} pre-existing prohibited artifact path(s) in {} — NOT blocking this {} gate.",
        advisory.len(),
        report.repo.display(),
        report.mode
    );
    match &report.baseline {
        Some(baseline) => eprintln!(
            "These paths are committed in the baseline {} ({}) and are untouched by this change, so they cannot enter its diff. This run did not create them (issue #1422).",
            baseline.rev,
            short_commit(&baseline.commit)
        ),
        None => eprintln!(
            "No baseline revision resolved ({}), so provenance could not be measured against a branch point; these tracked paths are unchanged in the worktree and are reported rather than blocked (issue #1422). Pass --baseline <rev> to measure this change precisely.",
            amplihack_utils::artifact_guard::BASELINE_CANDIDATES.join(", ")
        ),
    }
    eprintln!();
    for (root, group) in group_by_root(&advisory) {
        print_path_group(&root, &group);
    }
    eprintln!();
    eprintln!("Way forward (none of these is required to finish the current change):");
    let (removable, review) = split_removable(&advisory);
    for (root, group) in removable {
        // A file root (a coverage report) is ignored as itself, a directory
        // root as the directory.
        let ignore = if group.iter().all(|violation| violation.path == root) {
            root.clone()
        } else {
            format!("{root}/")
        };
        let untrack = git_rm_cached(&root, true);
        if is_literal_ignore_pattern(&root) {
            eprintln!(
                "  - Clean up in a dedicated change: {untrack} && echo '{ignore}' >> .gitignore && git commit -m 'chore: untrack {root}'"
            );
        } else {
            // A .gitignore line is a pattern, not a literal path; the guard
            // will not write one that could match more than this root.
            eprintln!(
                "  - Clean up in a dedicated change: {untrack} && git commit -m 'chore: untrack {root}'   # write its .gitignore entry by hand"
            );
        }
    }
    if !review.is_empty() {
        eprintln!(
            "  - Possibly source (matches a generic output directory name, or its name is unsafe in a shell command); the guard will not suggest removing it:"
        );
        print_review_remedy(&review);
    }
    eprintln!(
        "  - List every path and fail closed on the whole condition: amplihack hygiene artifact-guard --repo {} --mode all",
        report.repo.display()
    );
    eprintln!("  - Enforce it here anyway: --preexisting block (or {PREEXISTING_ENV}=block).");
    eprintln!();
}

fn print_path_group(root: &str, group: &[&ArtifactViolation]) {
    let rules = distinct_rules(group);
    // Escape the root and its `/` together: `"x"/` would read as a quoted
    // name followed by a stray slash.
    let root = display_path(&format!("{root}/"));
    let paths = distinct_paths(group);
    eprintln!("  {root}  — {} path(s), rule {rules}", paths.len());
    for path in paths.iter().take(ADVISORY_PATHS_PER_ROOT) {
        eprintln!("      {}", display_path(path));
    }
    if paths.len() > ADVISORY_PATHS_PER_ROOT {
        eprintln!(
            "      ... and {} more under {root}",
            paths.len() - ADVISORY_PATHS_PER_ROOT
        );
    }
}

/// Each distinct path in `group`, sorted. A path can be reported once per
/// source (staged and tracked); remedy output names it once.
fn distinct_paths<'a>(group: &[&'a ArtifactViolation]) -> BTreeSet<&'a str> {
    group.iter().map(|v| v.path.as_str()).collect()
}

/// The way forward for paths that may be source: an exact-path allowlist entry
/// where the path has a literal form the allowlist parser reads back as that
/// one path, manual review otherwise, and never a removal command.
fn print_review_remedy(review: &[&ArtifactViolation]) {
    let mut entries = BTreeSet::new();
    let mut unliteral = BTreeSet::new();
    let mut refused = BTreeSet::new();
    let mut omitted = 0;
    for (_, group) in group_by_root(review) {
        let paths = distinct_paths(&group);
        omitted += paths.len().saturating_sub(ADVISORY_PATHS_PER_ROOT);
        for path in paths.into_iter().take(ADVISORY_PATHS_PER_ROOT) {
            if !is_plain_path(path) {
                unliteral.insert(path);
            } else if !artifact_guard::is_valid_allowlist_entry(path) {
                refused.insert(path);
            } else {
                entries.insert(path);
            }
        }
    }
    if !entries.is_empty() {
        eprintln!(
            "  If a path is intentional source, add a reviewed entry to .amplihack-artifact-allowlist:"
        );
        for entry in entries {
            eprintln!("      {entry}");
        }
    }
    for path in unliteral {
        eprintln!(
            "  {path:?} needs manual review: it has no literal .amplihack-artifact-allowlist form; rename it instead."
        );
    }
    for path in refused {
        eprintln!(
            "  {path:?} cannot be allowlisted (root artifact names are refused by the allowlist parser); rename it or remove it in a reviewed change."
        );
    }
    if omitted > 0 {
        eprintln!(
            "  ... and {omitted} more path(s); list them with --mode all before allowlisting."
        );
    }
    eprintln!("  Otherwise remove it in a dedicated, reviewed change.");
}

/// Split violations into those with a removal root safe to print as a command
/// argument ([`is_command_safe`]), grouped by that root, and those that must
/// only ever be reviewed (no root, or a root that is not command-safe).
fn split_removable<'a>(
    violations: &[&'a ArtifactViolation],
) -> (
    BTreeMap<String, Vec<&'a ArtifactViolation>>,
    Vec<&'a ArtifactViolation>,
) {
    let mut removable: BTreeMap<String, Vec<&'a ArtifactViolation>> = BTreeMap::new();
    let mut review = Vec::new();
    for violation in violations {
        match violation.removal_root.as_deref() {
            Some(root) if is_removable(violation) => {
                removable
                    .entry(root.to_string())
                    .or_default()
                    .push(violation);
            }
            _ => review.push(*violation),
        }
    }
    (removable, review)
}

/// Whether `path` can be printed inside a single-quoted shell word without
/// ending the quoting or forging a report line. Inside single quotes the shell
/// treats every other character literally, `rm` takes its arguments as plain
/// paths, and [`git_rm_cached`] turns off git's pathspec reading, so nothing
/// else needs to be refused.
fn is_command_safe(path: &str) -> bool {
    !path.chars().any(|c| c == '\'' || c.is_control())
}

/// The `git rm --cached` command that untracks exactly `path` (recursively when
/// `recursive`). Git reads arguments as pathspecs even after `--`: a leading
/// `:` is pathspec magic (`':!src/node_modules'` means "everything except
/// that", so a plain `git rm -r --cached` would untrack the whole repository)
/// and `*`, `?`, `[` are globs. `--literal-pathspecs` makes git read `path` as
/// the one path it names. It is added only when `path` holds such a
/// character, so ordinary paths keep the established `git rm --cached` form.
/// `path` must be [`is_command_safe`].
fn git_rm_cached(path: &str, recursive: bool) -> String {
    let flags = if recursive { "-r --cached" } else { "--cached" };
    let git = if has_pathspec_magic(path) {
        "git --literal-pathspecs"
    } else {
        "git"
    };
    format!("{git} rm {flags} -- '{path}'")
}

/// Whether git would read `path` as anything but itself: a leading `:` (magic
/// signature) or a glob or escape character anywhere.
fn has_pathspec_magic(path: &str) -> bool {
    path.starts_with(':') || path.chars().any(|c| matches!(c, '*' | '?' | '[' | '\\'))
}

/// Whether `root` written to `.gitignore` is read as a plain name: no glob or
/// escape characters, no leading `!` (negation) or `#` (comment), and no
/// surrounding whitespace. A single-segment root such as `dist` stays
/// unanchored, as before #1536, so it also ignores nested `dist/` directories.
fn is_literal_ignore_pattern(root: &str) -> bool {
    root.trim() == root
        && !root.starts_with(['!', '#'])
        && !root.chars().any(|c| matches!(c, '*' | '?' | '[' | '\\'))
}

/// Whether the violating path is in the index, so `git rm --cached` can
/// untrack it. Untracked and ignored files can only be deleted.
fn is_indexed(violation: &ArtifactViolation) -> bool {
    matches!(
        violation.source,
        ArtifactSource::Staged | ArtifactSource::Tracked
    )
}

/// Whether `path` is `root` or beneath it, at a `/` boundary (`build` does not
/// contain `buildtools/a.rs`).
fn is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path` reads back from an allowlist line as exactly itself and is
/// safe to print as plain text: no glob characters, backslashes, quotes or
/// control characters, no leading `#`, and no surrounding whitespace.
fn is_plain_path(path: &str) -> bool {
    !path.is_empty()
        && path.trim() == path
        && !path.starts_with('#')
        && !path
            .chars()
            .any(|c| matches!(c, '*' | '?' | '\\' | '\'' | '"') || c.is_control())
}

/// `path` as plain text when [`is_plain_path`], otherwise escaped so a file
/// name cannot forge a report line or inject shell syntax.
fn display_path(path: &str) -> String {
    if is_plain_path(path) {
        path.to_string()
    } else {
        format!("{path:?}")
    }
}

fn short_commit(commit: &str) -> &str {
    let end = commit.len().min(12);
    &commit[..end]
}

/// Directory names that mark the top of a generated tree. A violation is
/// attributed to the shortest prefix ending in one of these, so 668 paths
/// collapse to one `node_modules` remedy instead of 668 allowlist lines.
const ARTIFACT_ROOT_COMPONENTS: &[&str] = &[
    "node_modules",
    "worktrees",
    "target",
    "dist",
    "build",
    "coverage",
    "out",
    "logs",
    "outputs",
    ".next",
    ".cache",
    ".npm",
    ".pnpm-store",
    ".turbo",
    ".parcel-cache",
    ".pytest_cache",
    "runtime",
    "session-state",
];

/// The display root a violation is grouped under: the shortest prefix ending in
/// an unambiguous root name (`node_modules`, `.cache`, ...). When the path holds
/// only generic names, the deepest one is used, so `apps/coverage/dist/main.js`
/// is grouped under `apps/coverage/dist`, never under the `apps/coverage`
/// project the guard exempted.
fn artifact_root(path: &str) -> String {
    let mut end = 0;
    let mut generic = None;
    for component in path.split('/') {
        end += component.len();
        if ARTIFACT_ROOT_COMPONENTS.contains(&component) {
            if !GENERIC_OUTPUT_SEGMENTS.contains(&component) {
                return path[..end].to_string();
            }
            generic = Some(end);
        }
        end += 1;
    }
    generic.map_or_else(|| path.to_string(), |end| path[..end].to_string())
}

fn group_by_root<'a>(
    violations: &[&'a ArtifactViolation],
) -> Vec<(String, Vec<&'a ArtifactViolation>)> {
    let mut grouped: BTreeMap<String, Vec<&'a ArtifactViolation>> = BTreeMap::new();
    for violation in violations {
        grouped
            .entry(artifact_root(&violation.path))
            .or_default()
            .push(violation);
    }
    grouped.into_iter().collect()
}

fn distinct_rules(violations: &[&ArtifactViolation]) -> String {
    let mut rules: Vec<&str> = violations
        .iter()
        .map(|violation| violation.rule_id.as_str())
        .collect();
    rules.sort_unstable();
    rules.dedup();
    rules.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_rm_reads_pathspec_magic_as_a_literal_path() {
        assert!(is_command_safe(":!x/node_modules"));
        assert_eq!(
            git_rm_cached(":!x/node_modules", true),
            "git --literal-pathspecs rm -r --cached -- ':!x/node_modules'"
        );
    }

    #[test]
    fn git_rm_reads_glob_characters_as_a_literal_path() {
        assert!(is_command_safe("build/[a]*.log"));
        assert_eq!(
            git_rm_cached("build/[a]*.log", false),
            "git --literal-pathspecs rm --cached -- 'build/[a]*.log'"
        );
    }

    #[test]
    fn git_rm_keeps_the_plain_form_for_ordinary_paths() {
        assert_eq!(
            git_rm_cached("node_modules", true),
            "git rm -r --cached -- 'node_modules'"
        );
        assert_eq!(
            git_rm_cached("a\\b/node_modules", true),
            "git --literal-pathspecs rm -r --cached -- 'a\\b/node_modules'"
        );
    }

    #[test]
    fn quotes_and_control_characters_are_not_command_safe() {
        assert!(!is_command_safe("it's/node_modules"));
        assert!(!is_command_safe("a\nb/node_modules"));
        assert!(is_command_safe("plain/node_modules"));
    }

    #[test]
    fn gitignore_entries_are_written_only_for_literal_roots() {
        assert!(is_literal_ignore_pattern("pkg/node_modules"));
        for root in [
            "pkg[1]/node_modules",
            "!keep/dist",
            "#c/dist",
            "a*/dist",
            "a\\b",
        ] {
            assert!(!is_literal_ignore_pattern(root), "{root}");
        }
    }

    #[test]
    fn untracked_and_tracked_violations_get_matching_commands() {
        let make = |source| ArtifactViolation {
            path: "build/new.log".to_string(),
            source,
            rule_id: "build-artifact".to_string(),
            remediation: String::new(),
            provenance: artifact_guard::ArtifactProvenance::Introduced,
            removal_root: Some("build/new.log".to_string()),
        };
        assert!(is_indexed(&make(ArtifactSource::Staged)));
        assert!(is_indexed(&make(ArtifactSource::Tracked)));
        assert!(!is_indexed(&make(ArtifactSource::Untracked)));
        assert!(!is_indexed(&make(ArtifactSource::IgnoredPresent)));
    }
}
