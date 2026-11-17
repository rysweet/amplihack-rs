//! Issue #1619: overriding the model tier of one named step.
//!
//! The recipe runner exports no step id, so amplihack cannot tell at launch
//! time which step a command belongs to. Instead, before the runner starts,
//! each recipe holding a named step is copied into a run-private directory
//! with that one `model:` line rewritten, and the copies go first in the
//! runner's search order.
//!
//! The patch is line-scoped and vocabulary-closed: the only value ever
//! written is one of three literal tier words, so no text from an override
//! file reaches a prompt or a shell body. Step ids are identifiers, never
//! path components, and a staged file's name comes only from the recipe
//! amplihack found.
//!
//! With nothing configured nothing is staged, no environment variable is set,
//! and the run behaves byte for byte as before.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::commands::launch::model_tiers::notice::Notice;
use crate::commands::launch::model_tiers::table::Tier;

/// The runner's first recipe search directory, and the one amplihack sets
/// nowhere else.
pub(crate) const RECIPE_DIR_ENV: &str = "AMPLIHACK_PACKAGE_RECIPE_DIR";

/// The run-private directory the patched copies live in.
const STAGING_DIR: &str = "recipe-overrides";

/// Longest accepted step id, in bytes.
const MAX_STEP_ID_BYTES: usize = 128;

/// Largest recipe staging will read. The largest bundled recipe is about
/// 54 KiB, so this leaves room to grow while keeping a search directory full
/// of large files from becoming an unbounded read.
pub(crate) const MAX_RECIPE_BYTES: u64 = 128 * 1024;

/// Most recipes staging will look at in one run. The bundle holds about 60.
pub(crate) const MAX_CANDIDATES: usize = 512;

/// The patched copies of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Staged {
    /// The directory to put first in the runner's search order.
    pub(crate) dir: PathBuf,
    /// Every file written, directly inside `dir`.
    pub(crate) files: Vec<PathBuf>,
}

/// What one run's staging produced: the patched copies, if any, and the lines
/// the caller prints about what it could not act on.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The patched copies, or `None` when nothing was staged.
    pub(crate) staged: Option<Staged>,
    /// Diagnostics, in order. The caller renders them: staging has no second
    /// output channel of its own.
    pub(crate) notices: Vec<Notice>,
}

/// Write patched copies of the recipes that declare any step in `overrides`.
///
/// `Outcome::staged` is `None` when there is nothing to do, as on every run
/// that configures no per-step tier. A step id that is not an identifier, and
/// one no candidate recipe declares, each stage nothing and come back as a
/// notice: neither may stop a run of a hundred agent steps, and neither may
/// pass in silence, since a mistyped step id has no other symptom. An
/// unreadable search directory is skipped without a line of its own, as the
/// runner would have skipped it too, and its step ids are reported unmatched.
pub(crate) fn stage(
    run_dir: &Path,
    recipe_dirs: &[PathBuf],
    top_level_recipe: &Path,
    overrides: &BTreeMap<String, Tier>,
) -> Result<Outcome, String> {
    let mut out = Outcome::default();
    let mut wanted: BTreeMap<&str, Tier> = BTreeMap::new();
    for (id, tier) in overrides {
        if validate_step_id(id) {
            wanted.insert(id.as_str(), *tier);
        } else {
            out.notices.push(Notice::BadStepId { step: id.clone() });
        }
    }
    if wanted.is_empty() {
        return Ok(out);
    }

    let dir = run_dir.join(STAGING_DIR);
    let mut staged = Staged {
        dir,
        files: Vec::new(),
    };
    let mut written: BTreeSet<String> = BTreeSet::new();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    for source in candidates(recipe_dirs, top_level_recipe) {
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if written.contains(&name) {
            continue;
        }
        let Some(text) = read_recipe(&source) else {
            continue;
        };
        let Some(patched) = patch(&text, &wanted) else {
            continue;
        };
        // The patch is line-scoped, and a recipe body can hold a line that
        // reads exactly like a step header at the step indent — a here-
        // document, say — which the line walk cannot tell from one. Such a
        // copy is no longer the recipe it was patched from, and the staging
        // directory goes first in the runner's search order, so writing it
        // would put a corrupted file in front of a sound one. Re-reading the
        // rewritten text as YAML is what tells the two apart.
        if let Err(reason) = reparses(&patched.text) {
            out.notices.push(Notice::StagedRecipeDropped {
                recipe: name.clone(),
                reason,
            });
            // The ids are counted as matched all the same: the recipe did
            // name them, and the drop is the line that explains this run.
            // Reporting them unmatched as well would say something untrue.
            matched.extend(patched.matched);
            written.insert(name);
            continue;
        }
        let target = staged.dir.join(&name);
        if target.parent() != Some(staged.dir.as_path()) {
            continue;
        }
        if staged.files.is_empty() {
            create_private_dir(&staged.dir)?;
        }
        write_private(&target, &patched.text)?;
        written.insert(name);
        matched.extend(patched.matched);
        staged.files.push(target);
    }
    for id in wanted.keys() {
        if !matched.contains(*id) {
            out.notices.push(Notice::StepTierUnmatched {
                step: (*id).to_string(),
            });
        }
    }
    out.staged = (!staged.files.is_empty()).then_some(staged);
    Ok(out)
}

/// Every recipe file worth looking at: those in the search directories
/// first, then the one named on the command line.
///
/// The search directories come first because a staged file answers every
/// later lookup of its name for the whole run. Staging the recipe named on
/// the command line ahead of them would let a repository-local file of a
/// bundled name stand in for the bundled one, bodies and all. The named
/// recipe is still staged where no search directory holds a file of that
/// name, so a single-step override covers the recipe the operator asked for
/// whatever the search path holds.
///
/// Capped at [`MAX_CANDIDATES`], so a directory of unbounded size does not
/// become an unbounded set of copies.
fn candidates(recipe_dirs: &[PathBuf], top_level_recipe: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in recipe_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            // A search directory that is not there is not an error: the
            // runner would have skipped it too.
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .collect();
        found.sort();
        out.extend(found);
    }
    out.truncate(MAX_CANDIDATES.saturating_sub(1));
    out.push(top_level_recipe.to_path_buf());
    out
}

/// One recipe's text, under the same bound every other amplihack file read
/// has. `None` for anything staging should pass over in silence, as the
/// runner would: a path that is not a readable regular file, and one over
/// [`MAX_RECIPE_BYTES`].
fn read_recipe(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_RECIPE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// `Ok(())` when the patched text is still YAML, or the place it stopped
/// being YAML.
///
/// The reason names the failure and where it is, never the text that caused
/// it: these lines land in run logs that get pasted into issues, and a recipe
/// body holds prompts and shell.
fn reparses(text: &str) -> Result<(), String> {
    match serde_yaml::from_str::<serde_yaml::Value>(text) {
        Ok(_) => Ok(()),
        Err(error) => Err(match error.location() {
            Some(at) => format!(
                "the rewritten copy no longer parses as YAML (line {}, column {})",
                at.line(),
                at.column()
            ),
            None => "the rewritten copy no longer parses as YAML".to_string(),
        }),
    }
}

/// One patched recipe, and which of the named steps it actually declared.
struct Patched {
    text: String,
    /// The step ids rewritten in this file. The caller diffs these against
    /// what was configured, so a step id no recipe declares is reported
    /// rather than passing in silence.
    matched: Vec<String>,
}

/// `text` with the `model:` of every named step rewritten or inserted, or
/// `None` when it declares none of them.
///
/// Read line by line, the way a reader of the file would: the step's `id:`
/// opens it, and the next `- ` at the same indentation closes it. Only the
/// `model:` line moves; prompts and shell bodies are copied verbatim.
///
/// A step id amplihack rewrites is read out of a line of `text`, so a file
/// that contains no wanted id as a substring declares no wanted step and the
/// line walk cannot change it. Saying so first keeps the 64 bundled recipes
/// this runs over — 22,000 lines, of which one file typically matches — a
/// substring scan rather than 22,000 string allocations thrown away.
fn patch(text: &str, wanted: &BTreeMap<&str, Tier>) -> Option<Patched> {
    if !wanted.keys().any(|id| text.contains(id)) {
        return None;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + wanted.len());
    let mut matched: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let Some((indent, id)) = step_id_line(line) else {
            out.push(line.to_string());
            index += 1;
            continue;
        };
        let Some(tier) = wanted.get(id.as_str()).copied() else {
            out.push(line.to_string());
            index += 1;
            continue;
        };
        let end = step_end(&lines, index, indent);
        let field_indent = " ".repeat(indent + 2);
        let model_line = format!("{field_indent}model: \"{}\"", tier.as_str());
        let id_position = out.len();
        let mut replaced = false;
        for line in &lines[index..end] {
            // Anchored to the step's own field indent, not to `model:`
            // anywhere in the body: a prompt or a shell line that happens to
            // begin `model:` is the step's content, and replacing it would
            // both corrupt the body and leave the step with no tier. No
            // bundled recipe has that shape today, so the invariant would
            // otherwise live in the data rather than in the code.
            if !replaced && is_step_field(line, indent, "model:") {
                out.push(model_line.clone());
                replaced = true;
            } else {
                out.push((*line).to_string());
            }
        }
        if !replaced {
            // A step that declares no model gains one directly after its id,
            // where every other field of the step already sits.
            out.insert(id_position + 1, model_line);
        }
        matched.push(id);
        index = end;
    }
    (!matched.is_empty()).then(|| Patched {
        text: out.join("\n"),
        matched,
    })
}

/// Whether `line` is the named field of a step whose `- id:` sits at
/// `indent`, so it is written at `indent + 2` like every other field.
fn is_step_field(line: &str, indent: usize, field: &str) -> bool {
    let trimmed = line.trim_start();
    line.len() - trimmed.len() == indent + 2 && trimmed.starts_with(field)
}

/// `(indent, id)` when `line` opens a step, as the bundled recipes write it:
/// `  - id: "step-08-implement"`.
fn step_id_line(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start().strip_prefix("- id:")?.trim();
    let id = rest.trim_matches(|c| c == '"' || c == '\'');
    (!id.is_empty() && !id.contains(char::is_whitespace)).then(|| (indent, id.to_string()))
}

/// The line after the last line of the step that starts at `start`.
fn step_end(lines: &[&str], start: usize, indent: usize) -> usize {
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") && line.len() - trimmed.len() <= indent {
            return offset;
        }
        if !trimmed.is_empty() && line.len() - trimmed.len() < indent {
            return offset;
        }
    }
    lines.len()
}

/// Whether `raw` is a step id amplihack will act on.
///
/// An identifier, never a path component: `.` and `..` are rejected along
/// with every separator, so a step id can never reach a path join as
/// anything but a name amplihack already matched inside a recipe.
pub(crate) fn validate_step_id(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > MAX_STEP_ID_BYTES {
        return false;
    }
    if !raw
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return false;
    }
    raw.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}
