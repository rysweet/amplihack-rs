//! Issue #1619: overriding the model tier of one named step.
//!
//! The step id never reaches the child process — the recipe runner sets no
//! step-id environment variable — so amplihack cannot know at launch time
//! which step a command belongs to. Patching the recipe is the honest way to
//! change one step's tier: before the runner starts, the recipes that contain
//! a named step are copied into a run-private directory with that one
//! `model:` line rewritten, and the copies are put first in the runner's
//! search order.
//!
//! The patch is line-scoped and vocabulary-closed: only one of three literal
//! words is ever written into a recipe file, so no text from an override file
//! can reach a prompt or a shell body. Step ids are identifiers, never path
//! components, and a staged file's name comes only from the recipe amplihack
//! found.
//!
//! With nothing configured, nothing is staged, no environment variable is set
//! and the run behaves byte for byte as it did before.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::commands::launch::model_tiers::table::Tier;

/// The runner's first recipe search directory, and the one amplihack sets
/// nowhere else.
pub(crate) const RECIPE_DIR_ENV: &str = "AMPLIHACK_PACKAGE_RECIPE_DIR";

/// The run-private directory the patched copies live in.
const STAGING_DIR: &str = "recipe-overrides";

/// Longest accepted step id, in bytes.
const MAX_STEP_ID_BYTES: usize = 128;

/// The patched copies of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Staged {
    /// The directory to put first in the runner's search order.
    pub(crate) dir: PathBuf,
    /// Every file written, directly inside `dir`.
    pub(crate) files: Vec<PathBuf>,
}

/// Write patched copies of the recipes that declare any step in `overrides`.
///
/// `Ok(None)` when there is nothing to do, which is the case for every run
/// that configures no per-step tier. A search directory that cannot be read,
/// a step id no recipe declares and a step id that is not an identifier are
/// all reported by staging nothing for them: none of them may stop a run of a
/// hundred agent steps.
pub(crate) fn stage(
    run_dir: &Path,
    recipe_dirs: &[PathBuf],
    top_level_recipe: &Path,
    overrides: &BTreeMap<String, Tier>,
) -> Result<Option<Staged>, String> {
    let wanted: BTreeMap<&str, Tier> = overrides
        .iter()
        .filter(|(id, _)| validate_step_id(id))
        .map(|(id, tier)| (id.as_str(), *tier))
        .collect();
    if wanted.is_empty() {
        return Ok(None);
    }

    let dir = run_dir.join(STAGING_DIR);
    let mut staged = Staged {
        dir,
        files: Vec::new(),
    };
    let mut written: BTreeSet<String> = BTreeSet::new();
    for source in candidates(recipe_dirs, top_level_recipe) {
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if written.contains(&name) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&source) else {
            continue;
        };
        let Some(patched) = patch(&text, &wanted) else {
            continue;
        };
        let target = staged.dir.join(&name);
        if target.parent() != Some(staged.dir.as_path()) {
            continue;
        }
        if staged.files.is_empty() {
            create_private_dir(&staged.dir)?;
        }
        write_private(&target, &patched)?;
        written.insert(name);
        staged.files.push(target);
    }
    if staged.files.is_empty() {
        return Ok(None);
    }
    Ok(Some(staged))
}

/// Every recipe file worth looking at: those in the search directories, and
/// the one named on the command line.
///
/// The named recipe is included even when it is outside every search
/// directory, so a single-step override always covers at least the recipe the
/// operator asked for.
fn candidates(recipe_dirs: &[PathBuf], top_level_recipe: &Path) -> Vec<PathBuf> {
    let mut out = vec![top_level_recipe.to_path_buf()];
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
    out
}

/// `text` with the `model:` of every named step rewritten or inserted, or
/// `None` when it declares none of them.
///
/// Read line by line, the way a reader of the file would: the step's `id:`
/// opens it, and the next `- ` at the same indentation closes it. Only the
/// `model:` line moves; prompts and shell bodies are copied verbatim.
fn patch(text: &str, wanted: &BTreeMap<&str, Tier>) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + wanted.len());
    let mut changed = false;
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
            if line.trim_start().starts_with("model:") && !replaced {
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
        changed = true;
        index = end;
    }
    changed.then(|| out.join("\n"))
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
