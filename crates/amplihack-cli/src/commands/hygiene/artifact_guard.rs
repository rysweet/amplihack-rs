use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use amplihack_utils::artifact_guard::{
    ArtifactGuardConfig, ArtifactGuardMode, ArtifactGuardReport, ArtifactViolation,
    PreExistingPolicy, scan_artifacts,
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
            violation.path,
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
    let shared: BTreeSet<String> = artifact_roots(&report.advisory_violations())
        .into_iter()
        .collect();
    for (root, group) in group_by_root(&blocking) {
        if shared.contains(&root) {
            for violation in &group {
                let path = &violation.path;
                eprintln!("  git rm --cached -- '{path}'   # drop from the change, keep on disk");
            }
            eprintln!(
                "  # (listed per path: '{root}' also holds pre-existing entries this change must not touch)"
            );
        } else {
            eprintln!("  git rm -r --cached -- '{root}'   # drop from the change, keep on disk");
            eprintln!("  rm -rf -- '{root}'               # or delete the leftover outright");
        }
    }
    eprintln!(
        "  amplihack hygiene artifact-guard --repo {} --mode {}   # re-run to confirm",
        report.repo.display(),
        report.mode
    );
    eprintln!();
    eprintln!("First violation detail:");
    if let Some(first) = blocking.first() {
        eprintln!("  {}: {}", first.path, first.remediation);
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
        let rules = distinct_rules(&group);
        eprintln!("  {root}/  — {} path(s), rule {rules}", group.len());
        for violation in group.iter().take(ADVISORY_PATHS_PER_ROOT) {
            eprintln!("      {}", violation.path);
        }
        if group.len() > ADVISORY_PATHS_PER_ROOT {
            eprintln!(
                "      ... and {} more under {root}/",
                group.len() - ADVISORY_PATHS_PER_ROOT
            );
        }
    }
    eprintln!();
    eprintln!("Way forward (none of these is required to finish the current change):");
    for root in artifact_roots(&advisory) {
        eprintln!(
            "  - Clean up in a dedicated change: git rm -r --cached -- '{root}' && echo '{root}/' >> .gitignore && git commit -m 'chore: untrack {root}'"
        );
    }
    eprintln!(
        "  - List every path and fail closed on the whole condition: amplihack hygiene artifact-guard --repo {} --mode all",
        report.repo.display()
    );
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
