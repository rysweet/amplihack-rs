//! Skill listing budget — integration guard for issue #1459.
//!
//! # Bug class
//!
//! Claude Code keeps the `name` + `description` pair of every installed skill
//! resident in every session, budgets that listing at a fraction of the context
//! window, and **silently** truncates the tail to bare names when it overruns.
//! Nothing warns — not the install, not the session. Measured on a clean
//! install, 67 of the 130 skills amplihack ships were reaching the model with
//! no description at all, alphabetically from `lawyer-analyst` onward, so more
//! than half of the corpus could not be matched to a task. Casualties included
//! `pr-guide`, `reviewing-code`, `testing-code`, `smart-test`, `qa-team`,
//! `verus-expert` and `oxidizer-workflow` — skills that plainly should be
//! selectable in this repository.
//!
//! A skill that is staged but unreachable is the same class of defect as a
//! skill that failed to stage, and `amplifier-bundle/context/PHILOSOPHY.md`
//! says install must fail loudly for the latter. These tests make the former
//! fail loudly too.
//!
//! # What is enforced
//!
//! | Test | Enforces |
//! | --- | --- |
//! | `I-01` | The walk finds exactly 130 `SKILL.md` files — no silent under-count. |
//! | `I-02` | The real bundle total is at or under `SKILL_LISTING_BUDGET_CHARS`. |
//! | `I-03` | The shell script and the Rust walk report the same total. |
//! | `I-04` | The install path calls the guard, in the documented order. |
//! | `I-05` | The pre-commit hook is wired so the failure arrives before CI. |
//!
//! `I-02` is the regression guard. It fails the moment the corpus grows back
//! past the limit, naming the skills that pushed it over.
//!
//! # Read-only invariant
//!
//! These tests only read files. They never write, remove, or create anything in
//! the repository.
//!
//! # Running
//!
//! ```bash
//! cargo test --test skill_description_budget -- --nocapture
//! ```
//!
//! Contract: `docs/reference/skill-listing-budget.md`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use amplihack_cli::skill_listing_budget::{
    SKILL_LISTING_BUDGET_CHARS, collect_skill_listing, over_budget_report,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The bundle ships this many `SKILL.md` files.
///
/// 130, not 129: `ls amplifier-bundle/skills | wc -l` counts 123 skill
/// directories plus six *category* directories, five of which hold nested
/// skills (`collaboration/` 1, `development/` 2, `meta-cognitive/` 1,
/// `quality/` 2, `research/` 1) and one of which — `common/` — holds shared
/// assets and no skill at all. The walk recurses, so all 130 are counted, and
/// nesting is preserved when the bundle is staged.
///
/// This is an equality, not a floor: a skill added without a description budget
/// in mind is exactly what #1459 was.
const EXPECTED_SKILL_COUNT: usize = 130;

static WORKSPACE_ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // bins/amplihack → bins/
    root.pop(); // bins/ → workspace root
    root
});

fn workspace_root() -> &'static Path {
    WORKSPACE_ROOT.as_path()
}

/// `amplifier-bundle/skills` — the real directory, never the repo-root `skills`
/// symlink, so no skill is reachable by two paths.
fn skills_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/skills")
}

fn budget_script() -> PathBuf {
    workspace_root().join("scripts/check-skill-description-budget.sh")
}

// ── I-01 ──────────────────────────────────────────────────────────────────────

/// I-01: the walk discovers exactly 130 `SKILL.md` files.
///
/// Without this, a broken or non-recursive walk would make every other
/// assertion here pass vacuously by measuring a smaller corpus — the
/// consistent-but-wrong number this whole check exists to prevent.
#[test]
fn i01_skill_corpus_is_fully_discovered() {
    let skills = skills_dir();
    assert!(
        skills.is_dir(),
        "amplifier-bundle/skills/ must exist at {}",
        skills.display()
    );

    let entries = collect_skill_listing(&skills).expect("collect_skill_listing on the bundle");

    assert_eq!(
        entries.len(),
        EXPECTED_SKILL_COUNT,
        "expected {EXPECTED_SKILL_COUNT} SKILL.md files under {}, found {}. \
         If a skill was deliberately added or removed, update EXPECTED_SKILL_COUNT \
         in the same commit; if not, the recursive walk is broken.",
        skills.display(),
        entries.len()
    );

    // The seven nested skills are the ones a non-recursive walk drops, and two
    // of them were named casualties in #1459.
    for nested in ["reviewing-code", "testing-code"] {
        assert!(
            entries.iter().any(|e| e.name == nested),
            "nested skill {nested} must be discovered — the walk must recurse"
        );
    }

    assert!(
        entries.iter().all(|e| e.chars > 0),
        "every bundled skill must contribute a countable name and description; \
         zero-character entries mean malformed frontmatter: {:?}",
        entries.iter().filter(|e| e.chars == 0).collect::<Vec<_>>()
    );
}

// ── I-02 ──────────────────────────────────────────────────────────────────────

/// I-02: the bundled listing is at or under the budget. **This is the guard.**
///
/// It fails the moment the corpus drifts back over the limit, printing the full
/// report so the failure names the skills to shorten rather than just the
/// number. Do not raise `SKILL_LISTING_BUDGET_CHARS` to make it pass — the
/// limit sits about 5% below a measured truncation point, and raising it buys
/// nothing except silent truncation again.
#[test]
fn i02_bundled_skill_listing_is_under_budget() {
    let entries =
        collect_skill_listing(&skills_dir()).expect("collect_skill_listing on the bundle");

    // Non-vacuous: an empty or short listing must fail here rather than sail
    // under the budget by measuring nothing.
    assert_eq!(
        entries.len(),
        EXPECTED_SKILL_COUNT,
        "refusing to report 'under budget' from a listing of {} skills (expected {EXPECTED_SKILL_COUNT})",
        entries.len()
    );

    let total: usize = entries.iter().map(|e| e.chars).sum();
    println!(
        "skill listing: {total} / {SKILL_LISTING_BUDGET_CHARS} characters across {} skills",
        entries.len()
    );

    if let Some(report) = over_budget_report(&entries, SKILL_LISTING_BUDGET_CHARS) {
        panic!(
            "the bundled skill listing is over budget by {} characters \
             ({total} of {SKILL_LISTING_BUDGET_CHARS}).\n\
             Claude Code truncates the tail of this listing to bare names, so the skills \
             past the cut reach the model with no description and cannot be matched to a task.\n\
             Shorten the `description` frontmatter of the skills below to about 120 characters.\n\n\
             {report}",
            total.saturating_sub(SKILL_LISTING_BUDGET_CHARS)
        );
    }

    println!(
        "headroom: {} characters",
        SKILL_LISTING_BUDGET_CHARS.saturating_sub(total)
    );
}

// ── I-03 ──────────────────────────────────────────────────────────────────────

/// I-03: the shell script and the Rust walk report the same total.
///
/// The script is what produces the number quoted in a PR and what runs in
/// pre-commit; the Rust walk is what fails an install. Two counters that
/// disagree are worse than one — the PR would quote a number no test enforces.
/// Both trim each value before measuring, and this is the assertion that keeps
/// them honest about it.
#[cfg(unix)]
#[test]
fn i03_script_and_library_totals_agree() {
    use std::process::Command;

    let script = budget_script();
    assert!(
        script.is_file(),
        "the re-measurement script must exist at {} — it is how the number in the \
         PR body and the pre-commit hook are produced",
        script.display()
    );

    let output = Command::new("bash")
        .arg(&script)
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|err| panic!("failed to run {}: {err}", script.display()));

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let (script_total, script_count) = parse_script_totals(&combined).unwrap_or_else(|| {
        panic!(
            "could not parse a total out of the script's output — it must print a line \
             like `skill listing budget: 15801 / 20000 characters (130 skills, ...)`:\n{combined}"
        )
    });

    let entries =
        collect_skill_listing(&skills_dir()).expect("collect_skill_listing on the bundle");
    let rust_total: usize = entries.iter().map(|e| e.chars).sum();

    assert_eq!(
        script_count,
        entries.len(),
        "script counted {script_count} skills, the library counted {} — the two walks \
         disagree about the corpus:\n{combined}",
        entries.len()
    );
    assert_eq!(
        script_total, rust_total,
        "script reports {script_total} characters, the library reports {rust_total}. \
         Both must count trimmed `name` + `description` bytes over the same files:\n{combined}"
    );

    assert!(
        output.status.success(),
        "the script must exit 0 while the corpus is under budget:\n{combined}"
    );
}

/// Pull `(total, skill_count)` out of a line like
/// `skill listing budget: 15801 / 20000 characters (130 skills, 4199 to spare)`.
///
/// Tolerates the `EXCEEDED` variant and any trailing wording, so the test pins
/// the numbers rather than the prose.
#[cfg(unix)]
fn parse_script_totals(output: &str) -> Option<(usize, usize)> {
    let line = output
        .lines()
        .find(|l| l.contains("skill listing budget"))?;
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let slash = tokens.iter().position(|t| *t == "/")?;
    let total: usize = tokens.get(slash.checked_sub(1)?)?.parse().ok()?;
    let skills_at = tokens.iter().position(|t| t.starts_with("skills"))?;
    let count: usize = tokens
        .get(skills_at.checked_sub(1)?)?
        .trim_start_matches('(')
        .parse()
        .ok()?;
    Some((total, count))
}

// ── I-04 ──────────────────────────────────────────────────────────────────────

/// I-04: install calls the guard, in the documented order.
///
/// A budget module nothing calls is a measurement, not a guard. The ordering is
/// load-bearing, not stylistic: the budget check treats a missing staged skills
/// root as an empty listing, which passes on its own. That is only safe because
/// `verify_skill_count` runs first and has already recorded the absence.
#[test]
fn i04_install_verification_calls_the_budget_guard() {
    let verification =
        workspace_root().join("crates/amplihack-cli/src/commands/install/verification.rs");
    let source = std::fs::read_to_string(&verification)
        .unwrap_or_else(|err| panic!("read {}: {err}", verification.display()));

    let body = source
        .split_once("fn verify_install_completeness")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| {
            panic!(
                "verify_install_completeness missing from {}",
                verification.display()
            )
        });

    let count_at = body
        .find("verify_skill_count(")
        .expect("verify_install_completeness must call verify_skill_count");
    let budget_at = body
        .find("verify_skill_description_budget(")
        .unwrap_or_else(|| {
            panic!(
                "verify_install_completeness must call verify_skill_description_budget — \
             without it the budget is measured in tests but never enforced at install time \
             ({})",
                verification.display()
            )
        });
    let bundle_at = body
        .find("verify_staged_bundle(")
        .expect("verify_install_completeness must call verify_staged_bundle");

    assert!(
        count_at < budget_at && budget_at < bundle_at,
        "verify_skill_description_budget must be called AFTER verify_skill_count \
         (which reports an absent staged tree) and BEFORE verify_staged_bundle"
    );

    // It must measure the amplihack-staged tree, never the user's own
    // ~/.claude/skills — a user with 200 personal skills must not be locked out
    // of installing amplihack.
    assert!(
        !source.contains("~/.claude/skills"),
        "the budget guard must measure <claude_dir>/skills only, never the user's own skills tree"
    );
}

// ── I-05 ──────────────────────────────────────────────────────────────────────

/// I-05: the script runs in pre-commit.
///
/// Same reasoning as `scripts/check-brick-budget.sh`: the CI test is the
/// authority, the hook only moves the feedback from thirteen minutes later to
/// about a second later.
#[test]
fn i05_budget_script_is_wired_into_pre_commit() {
    let config = workspace_root().join(".pre-commit-config.yaml");
    let source = std::fs::read_to_string(&config)
        .unwrap_or_else(|err| panic!("read {}: {err}", config.display()));

    assert!(
        source.contains("scripts/check-skill-description-budget.sh"),
        "the budget script must be wired into {} next to check-brick-budget",
        config.display()
    );

    let script = budget_script();
    assert!(
        script.is_file(),
        "{} is referenced by pre-commit but does not exist",
        script.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&script)
            .unwrap_or_else(|err| panic!("stat {}: {err}", script.display()))
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "{} must be executable, or pre-commit's `language: system` entry cannot run it",
            script.display()
        );
    }
}
