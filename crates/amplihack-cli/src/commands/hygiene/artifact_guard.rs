use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use amplihack_utils::artifact_guard::{
    ArtifactGuardConfig, ArtifactGuardMode, ArtifactGuardReport, ArtifactViolation,
    PreExistingPolicy, RUN_WORKTREE_GITLINK, is_at_or_under, is_in_reserved_worktrees_dir,
    scan_artifacts,
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

/// Printed after a configuration error raised by the scan itself.
const SCAN_ERROR_HINT: &str =
    "Fix the repository path, Git state, mode, allowlist, or baseline and rerun the guard.";

/// Issue #1528: the extra remediation bullet, printed only when a run-worktree
/// gitlink blocks.
const GITLINK_BULLET: &str = "  - A run worktree is a live checkout recorded as a gitlink: drop it from the index only, never delete it, and commit from inside the PR worktree. The allowlist does not apply to run-worktree-gitlink.";

const GITLINK_LINE_SUFFIX: &str =
    "   # index only: keeps the worktree and its files; -f only overrides git's up-to-date check";

const NEVER_DELETE_REGISTERED: &str = "it holds registered worktree(s) — see git worktree list";
const NEVER_DELETE_RESERVED: &str =
    "it is reserved for run worktrees — inspect it and remove leftovers by hand";

pub fn run(args: HygieneArtifactGuardArgs) -> Result<()> {
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir().map_err(|error| {
            eprintln!(
                "Artifact Guard configuration error: resolve current directory: {}",
                escape(&error.to_string())
            );
            command_error::exit_error(2)
        })?,
    };

    let mode = match ArtifactGuardMode::parse(&args.mode) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!(
                "Artifact Guard configuration error: {}",
                escape(&error.to_string())
            );
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
            eprintln!(
                "Artifact Guard configuration error: {}",
                escape(&error.to_string())
            );
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
                escape(&repo_text(&report)),
                report.mode
            );
            Ok(())
        }
        Err(error) => {
            // The error can carry Git's stderr or repository content.
            eprintln!(
                "Artifact Guard configuration error: {}",
                escape(&error.to_string())
            );
            eprintln!("{SCAN_ERROR_HINT}");
            Err(command_error::exit_error(2))
        }
    }
}

fn print_violation_report(report: &ArtifactGuardReport) {
    let blocking = report.blocking_violations();
    let (gitlinks, others): (Vec<&ArtifactViolation>, Vec<&ArtifactViolation>) = blocking
        .iter()
        .copied()
        .partition(|violation| violation.rule_id == RUN_WORKTREE_GITLINK);
    let scope = if report.preexisting_is_advisory() {
        " introduced by this change"
    } else {
        ""
    };
    eprintln!(
        "Artifact Guard blocked {} prohibited artifact path(s){scope} in {} (mode: {}).",
        blocking.len(),
        escape(&repo_text(report)),
        report.mode
    );
    if let Some(baseline) = &report.baseline {
        eprintln!(
            "Baseline: {} ({}).",
            escape(&baseline.rev),
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
            escape(&violation.path),
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
    if !gitlinks.is_empty() {
        eprintln!("{GITLINK_BULLET}");
    }
    eprintln!();
    eprintln!("Exact commands for the paths above:");
    // Issue #1528: a gitlink is a live checkout. Plain `git rm --cached` fails
    // on one whose worktree moved on, and grouping it under `worktrees` would
    // print a delete for every run's work, so each gets its own index-only line.
    for violation in &gitlinks {
        match pathspec(&violation.path) {
            Some(spec) => eprintln!("  git rm --cached -f -- {spec}{GITLINK_LINE_SUFFIX}"),
            None => eprintln!("  {}", no_command_comment(&violation.path)),
        }
    }
    // Collapsing to an artifact root is only safe when the change owns the
    // whole root. If a pre-existing tree shares it -- the issue #1422 shape,
    // where one new package lands inside 668 vendored ones -- a
    // `git rm -r --cached node_modules` would untrack all 668 as a side effect,
    // which is precisely the unrelated change the run was told not to make.
    let shared: BTreeSet<String> = artifact_roots(&report.advisory_violations())
        .into_iter()
        .collect();
    for (root, group) in group_by_root(&others) {
        if shared.contains(&root) {
            for violation in &group {
                match pathspec(&violation.path) {
                    Some(spec) => eprintln!(
                        "  git rm --cached -- {spec}   # drop from the change, keep on disk"
                    ),
                    None => eprintln!("  {}", no_command_comment(&violation.path)),
                }
            }
            eprintln!(
                "  # (listed per path: '{}' also holds pre-existing entries this change must not touch)",
                escape(&root)
            );
            continue;
        }
        let Some(spec) = pathspec(&root) else {
            eprintln!("  {}", no_command_comment(&root));
            continue;
        };
        eprintln!("  git rm -r --cached -- {spec}   # drop from the change, keep on disk");
        // Issue #1528: never print a delete for a root that can hold a run's
        // uncommitted work.
        match never_delete_reason(&root, &report.registered_worktrees) {
            Some(reason) => eprintln!("  # never delete {}: {reason}", shell_quote(&root)),
            None => eprintln!(
                "  rm -rf -- {}               # or delete the leftover outright",
                shell_quote(&root)
            ),
        }
    }
    if let Some(repo) = safe_repo_arg(report) {
        eprintln!(
            "  amplihack hygiene artifact-guard --repo {repo} --mode {}   # re-run to confirm",
            report.mode
        );
    }
    eprintln!();
    eprintln!("First violation detail:");
    if let Some(first) = blocking.first() {
        eprintln!("  {}: {}", escape(&first.path), first.remediation);
    }
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
        escape(&repo_text(report)),
        report.mode
    );
    match &report.baseline {
        Some(baseline) => eprintln!(
            "These paths are committed in the baseline {} ({}) and are untouched by this change, so they cannot enter its diff. This run did not create them (issue #1422).",
            escape(&baseline.rev),
            short_commit(&baseline.commit)
        ),
        None => eprintln!(
            "No baseline revision resolved ({}), so provenance could not be measured against a branch point; these tracked paths are unchanged in the worktree and are reported rather than blocked (issue #1422). Pass --baseline <rev> to measure this change precisely.",
            amplihack_utils::artifact_guard::BASELINE_CANDIDATES.join(", ")
        ),
    }
    eprintln!();
    for (root, group) in group_by_root(&advisory) {
        let rules = distinct_rules(&group);
        let shown = escape(&root);
        eprintln!("  {shown}/  — {} path(s), rule {rules}", group.len());
        for violation in group.iter().take(ADVISORY_PATHS_PER_ROOT) {
            eprintln!("      {}", escape(&violation.path));
        }
        if group.len() > ADVISORY_PATHS_PER_ROOT {
            eprintln!(
                "      ... and {} more under {shown}/",
                group.len() - ADVISORY_PATHS_PER_ROOT
            );
        }
    }
    eprintln!();
    eprintln!("Way forward (none of these is required to finish the current change):");
    for root in artifact_roots(&advisory) {
        if let Some(line) = cleanup_command(&root) {
            eprintln!("  - Clean up in a dedicated change: {line}");
        }
    }
    if let Some(repo) = safe_repo_arg(report) {
        eprintln!(
            "  - List every path and fail closed on the whole condition: amplihack hygiene artifact-guard --repo {repo} --mode all"
        );
    }
    eprintln!("  - Enforce it here anyway: --preexisting block (or {PREEXISTING_ENV}=block).");
    eprintln!();
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

fn artifact_root(path: &str) -> String {
    let mut prefix: Vec<&str> = Vec::new();
    for component in path.split('/') {
        prefix.push(component);
        if ARTIFACT_ROOT_COMPONENTS.contains(&component) {
            return prefix.join("/");
        }
    }
    path.to_string()
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

fn artifact_roots(violations: &[&ArtifactViolation]) -> Vec<String> {
    group_by_root(violations)
        .into_iter()
        .map(|(root, _)| root)
        .collect()
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

/// The one-commit clean-up for a pre-existing `root`, runnable as printed.
/// The `.gitignore` entry is added only when `root` means itself there: a
/// leading `#` or `!`, a glob or a backslash would change what it ignores.
fn cleanup_command(root: &str) -> Option<String> {
    let spec = pathspec(root)?;
    let mut line = format!("git rm -r --cached -- {spec}");
    let gitignore_literal = !root.starts_with(['#', '!'])
        && !root.contains(['*', '?', '[', '\\'])
        && !root.ends_with(' ');
    if gitignore_literal {
        line.push_str(&format!(
            " && echo {} >> .gitignore && git add .gitignore",
            shell_quote(&format!("{root}/"))
        ));
    }
    line.push_str(&format!(
        " && git commit -m {}",
        shell_quote(&format!("chore: untrack {root}"))
    ));
    Some(line)
}

/// Why `root` must never get an `rm -rf` (issue #1528): it holds (or sits in)
/// a registered worktree, or its first component is `worktrees` in any ASCII
/// case, where an unregistered directory can still hold a run's work.
fn never_delete_reason(root: &str, registered: &[String]) -> Option<&'static str> {
    if registered
        .iter()
        .any(|worktree| is_at_or_under(worktree, root) || is_at_or_under(root, worktree))
    {
        return Some(NEVER_DELETE_REGISTERED);
    }
    is_in_reserved_worktrees_dir(root).then_some(NEVER_DELETE_RESERVED)
}

fn no_command_comment(path: &str) -> String {
    format!(
        "# no command for {}: the path contains an unsafe character; inspect it and fix it by hand",
        escape(path)
    )
}

fn repo_text(report: &ArtifactGuardReport) -> String {
    report.repo.display().to_string()
}

/// `--repo` for a printed command line, or `None` when the path holds an
/// unsafe character and no command may embed it.
fn safe_repo_arg(report: &ArtifactGuardReport) -> Option<String> {
    let repo = repo_text(report);
    (!has_unsafe_char(&repo)).then(|| shell_quote(&repo))
}

/// A shell-quoted Git pathspec for `path`, or `None` when the path holds an
/// unsafe character. `:(literal)` is added only when Git would otherwise read
/// the path as a pattern or as pathspec magic, so `wt*` never matches `wtA`.
fn pathspec(path: &str) -> Option<String> {
    if has_unsafe_char(path) {
        return None;
    }
    let is_pattern = path.starts_with(':') || path.contains(['*', '?', '[', '\\']);
    Some(if is_pattern {
        shell_quote(&format!(":(literal){path}"))
    } else {
        shell_quote(path)
    })
}

/// Single-quote `value` for bash; a `'` becomes `'\''`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Control, bidirectional and invisible format characters, plus the
/// replacement character that stands in for bytes that were not UTF-8. None
/// of them may reach the terminal raw or a printed command.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
                | '\u{fffd}'
        )
}

fn has_unsafe_char(value: &str) -> bool {
    value.chars().any(is_unsafe_char)
}

/// Escape unsafe characters for display: `\n`, `\t`, `\r`, otherwise `\u{hex}`.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => escaped.push_str(r"\n"),
            '\t' => escaped.push_str(r"\t"),
            '\r' => escaped.push_str(r"\r"),
            c if is_unsafe_char(c) => escaped.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => escaped.push(c),
        }
    }
    escaped
}
