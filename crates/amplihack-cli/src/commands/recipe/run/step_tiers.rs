//! Issue #1619: overriding the model tier of one named step.
//!
//! The recipe runner exports no step id, so amplihack cannot tell at launch
//! time which step a command belongs to. Instead, before the runner starts,
//! each recipe holding a named step is copied into a run-private directory
//! with that one `model:` line rewritten, and `execute.rs` passes that
//! directory to the runner as its first `-R` argument. `-R` is the only route
//! that wins: the runner resolves a recipe from its `-R` directories first,
//! then an anchored search, then `AMPLIHACK_PACKAGE_RECIPE_DIR`, which
//! amplihack's own bundle `-R` already outranked.
//!
//! The patch is line-scoped and vocabulary-closed: the only value ever
//! written is one of three literal tier words, so no text from an override
//! file reaches a prompt or a shell body. Step ids are identifiers, never
//! path components, and a staged file's name comes only from the recipe
//! amplihack found. Only agent steps are patched. With nothing configured
//! nothing is staged and the run behaves byte for byte as before.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::commands::launch::model_tiers::notice::Notice;
use crate::commands::launch::model_tiers::table::Tier;

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
    /// The directory the caller passes as the runner's first `-R`.
    pub(crate) dir: PathBuf,
    /// Every file written, directly inside `dir`.
    pub(crate) files: Vec<PathBuf>,
    /// The staged copy of the recipe named on the command line, set only
    /// when the file staged under that name is that same file.
    ///
    /// A `-R` directory is searched for a *name*, and the recipe named on
    /// the command line is passed to the runner as a path, so `-R` alone
    /// never reaches it. The caller substitutes this path for the positional
    /// argument, which is the only way a single-step override covers a step
    /// of the top-level recipe itself.
    pub(crate) top_level: Option<PathBuf>,
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
/// `Outcome::staged` is `None` when there is nothing to do. A step id that is
/// not an identifier, and one no candidate recipe declares, each stage nothing
/// and come back as a notice: neither may stop a run, and neither may pass in
/// silence, since a mistyped step id has no other symptom. An unreadable
/// search directory is skipped as the runner would have skipped it.
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
        top_level: None,
    };
    // Computed before the loop rather than accumulated during it: a candidate
    // that returns early reserves nothing, so a set built as a side effect of
    // staging would leave exactly those names free for the recipe named on
    // the command line to take (issue #1619, S1 in review).
    let search_names = search_dir_names(recipe_dirs);
    let mut written: BTreeSet<String> = BTreeSet::new();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let (sources, truncated) = candidates(recipe_dirs, top_level_recipe);
    if truncated {
        out.notices.push(Notice::RecipeCandidatesCapped {
            cap: MAX_CANDIDATES,
        });
    }
    for source in sources {
        let Some(name) = source
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };
        if written.contains(&name) {
            continue;
        }
        let Some(text) = read_recipe(&source.path) else {
            continue;
        };
        let Some(patched) = patch(&text, &wanted) else {
            continue;
        };
        for step in &patched.not_an_agent {
            // The id exists, so it is not a typo; `model:` on any other kind
            // of step is a field the runner ignores.
            out.notices.push(Notice::StepTierNotAnAgentStep {
                recipe: name.clone(),
                step: step.clone(),
            });
        }
        matched.extend(patched.not_an_agent);
        if patched.matched.is_empty() {
            continue;
        }
        // R1: a staged copy answers every later lookup of its name for the
        // whole run, so staging the command line's recipe under a name a
        // search directory also holds would let a repository-local file stand
        // in for the bundled one, bodies and all. The same file reached by
        // two routes is not a collision.
        if !source.from_search_dir
            && search_names
                .get(&name)
                .is_some_and(|held| !same_file(held, &source.path))
        {
            out.notices.push(Notice::StagedRecipeShadowed {
                recipe: name.clone(),
            });
            matched.extend(patched.matched);
            continue;
        }
        // A recipe body can hold a line that reads exactly like a step header
        // at the step indent — a here-document, say — which the line walk
        // cannot tell from one. The staging directory is the runner's first
        // `-R`, so writing such a copy would put a corrupted file in front of
        // a sound one. Re-reading it as YAML tells the two apart.
        if let Err(reason) = reparses(&patched.text) {
            out.notices.push(Notice::StagedRecipeDropped {
                recipe: name.clone(),
                reason,
            });
            // Counted as matched all the same: the recipe did name them, and
            // reporting them unmatched too would say something untrue.
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
        // Set whichever route reached it: the top-level recipe is usually
        // itself resolved out of a search directory, and the positional
        // argument must still be redirected to that copy.
        if same_file(&source.path, top_level_recipe) {
            staged.top_level = Some(target.clone());
        }
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

/// Whether `a` and `b` name the same file on disk.
///
/// Fails closed: a path that cannot be canonicalised counts as a different
/// file, so the caller refuses to stage rather than assuming a match.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The first file of each name across the search directories, in the order
/// the runner would consult them.
///
/// Bounded by [`MAX_CANDIDATES`] over the same order as the candidate list, so
/// a name this map does not reach is also a file staging will not look at, and
/// the run says so through [`Notice::RecipeCandidatesCapped`].
fn search_dir_names(recipe_dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut out: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in recipe_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_recipe_file(path))
            .collect();
        found.sort();
        for path in found {
            if out.len() >= MAX_CANDIDATES {
                return out;
            }
            if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) {
                out.entry(name).or_insert(path);
            }
        }
    }
    out
}

/// One recipe staging may patch, and which route reached it.
struct Candidate {
    path: PathBuf,
    /// `false` for the recipe named on the command line, which is the only
    /// one that may collide with a name a search directory owns.
    from_search_dir: bool,
}

/// Every recipe file worth looking at: those in the search directories
/// first, then the one named on the command line, and whether the search
/// directories held more than the cap.
///
/// The search directories come first because a staged file answers every later
/// lookup of its name for the whole run, so staging the command line's recipe
/// ahead of them would let a repository-local file of a bundled name stand in
/// for the bundled one. It is still staged where no search directory holds
/// that name. Capped at [`MAX_CANDIDATES`].
fn candidates(recipe_dirs: &[PathBuf], top_level_recipe: &Path) -> (Vec<Candidate>, bool) {
    let mut out: Vec<Candidate> = Vec::new();
    for dir in recipe_dirs {
        // A search directory that is not there is not an error: the runner
        // would have skipped it too.
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_recipe_file(path))
            .collect();
        found.sort();
        out.extend(found.into_iter().map(|path| Candidate {
            path,
            from_search_dir: true,
        }));
    }
    let room = MAX_CANDIDATES.saturating_sub(1);
    let truncated = out.len() > room;
    out.truncate(room);
    out.push(Candidate {
        path: top_level_recipe.to_path_buf(),
        from_search_dir: false,
    });
    (out, truncated)
}

/// Whether `path` is a regular file the runner would read as a recipe.
fn is_recipe_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "yaml" || ext == "yml")
        && path.is_file()
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
/// it: a recipe body holds prompts and shell, and run logs get pasted into
/// issues.
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
    /// The rewritten recipe. Where `matched` is empty nothing was rewritten
    /// and this is the original text; the caller does not write it.
    text: String,
    /// The step ids rewritten in this file. The caller diffs these against
    /// what was configured, so a step id no recipe declares is reported
    /// rather than passing in silence.
    matched: Vec<String>,
    /// Named ids this recipe declares as something other than an agent step.
    /// Nothing is rewritten for them, but the id exists, so the caller
    /// reports it as its own case rather than as one nothing declares.
    not_an_agent: Vec<String>,
}

/// Whether a parsed step is an agent step, by the runner's own rule
/// (`Step::effective_type`, `models.rs`): the declared `type:` if there is
/// one, else a `recipe:` makes it a sub-recipe, an `agent:` makes it an
/// agent, a `prompt:` without a `command:` makes it an agent, and anything
/// else is bash.
fn is_agent_step(step: &serde_yaml::Value) -> bool {
    if let Some(declared) = step.get("type").and_then(serde_yaml::Value::as_str) {
        return declared == "agent";
    }
    if step.get("recipe").is_some() {
        return false;
    }
    if step.get("agent").is_some() {
        return true;
    }
    step.get("prompt").is_some() && step.get("command").is_none()
}

/// Every step `text` declares, by id, and whether it is an agent step.
///
/// `None` when the text is not a recipe amplihack can read as YAML. The step
/// type is never inferred from the line walk: the runner infers it from fields
/// that may sit anywhere in the step.
fn declared_steps(text: &str) -> Option<BTreeMap<String, bool>> {
    let document: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let steps = document.get("steps")?.as_sequence()?;
    let mut out = BTreeMap::new();
    for step in steps {
        let Some(id) = step.get("id").and_then(serde_yaml::Value::as_str) else {
            continue;
        };
        out.insert(id.to_string(), is_agent_step(step));
    }
    Some(out)
}

/// `text` with the `model:` of every named agent step rewritten or inserted,
/// or `None` when it declares none of the named steps at all.
///
/// Read line by line, the way a reader of the file would: the step's `id:`
/// opens it, and the next `- ` at the same indentation closes it. Only the
/// `model:` line moves; prompts and shell bodies are copied verbatim. Which
/// steps it may write to comes from a parse of the original text.
///
/// The substring test first keeps the 64 bundled recipes this runs over, of
/// which one file typically matches, a scan rather than 64 YAML parses.
fn patch(text: &str, wanted: &BTreeMap<&str, Tier>) -> Option<Patched> {
    if !wanted.keys().any(|id| text.contains(id)) {
        return None;
    }
    let declared = declared_steps(text)?;
    let mut agents: BTreeMap<&str, Tier> = BTreeMap::new();
    let mut not_an_agent: Vec<String> = Vec::new();
    for (id, tier) in wanted {
        match declared.get(*id) {
            Some(true) => {
                agents.insert(id, *tier);
            }
            Some(false) => not_an_agent.push((*id).to_string()),
            None => {}
        }
    }
    if agents.is_empty() {
        return (!not_an_agent.is_empty()).then(|| Patched {
            text: text.to_string(),
            matched: Vec::new(),
            not_an_agent,
        });
    }
    let wanted = &agents;
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
            // anywhere in the body: a prompt or shell line beginning
            // `model:` is the step's content, and replacing it would corrupt
            // the body and leave the step with no tier.
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
    (!matched.is_empty() || !not_an_agent.is_empty()).then(|| Patched {
        text: out.join("\n"),
        matched,
        not_an_agent,
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
