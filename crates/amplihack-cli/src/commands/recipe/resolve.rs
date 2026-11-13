use super::*;
use crate::commands::install::bundle_compat::is_incompatible_smart_orchestrator;

pub(super) fn resolve_path_from(base_dir: &Path, path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let display = path.display().to_string();
    if display.trim().is_empty() {
        anyhow::bail!("Path cannot be empty");
    }

    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(base_dir.join(path))
}

pub(crate) fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|existing| existing == &candidate) {
        paths.push(candidate);
    }
}

fn resolve_env_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path,
    }
}

pub(super) fn amplihack_home_recipe_dir() -> Option<PathBuf> {
    let amplihack_home = amplihack_home_root()?;

    let candidate = amplihack_home.join("amplifier-bundle").join("recipes");
    if candidate.is_dir() {
        return Some(candidate);
    }

    tracing::warn!(
        amplihack_home = %amplihack_home.display(),
        searched = %candidate.display(),
        "AMPLIHACK_HOME root does not contain a usable amplifier-bundle/recipes directory; ignoring for recipe discovery"
    );
    None
}

pub(super) fn amplihack_home_root() -> Option<PathBuf> {
    let raw = std::env::var_os("AMPLIHACK_HOME")?;
    if raw.is_empty() {
        return None;
    }

    let amplihack_home = resolve_env_path(PathBuf::from(&raw));
    if !amplihack_home.is_dir() {
        tracing::warn!(
            amplihack_home = %amplihack_home.display(),
            "AMPLIHACK_HOME set but resolved path is not a directory; ignoring for recipe discovery"
        );
        return None;
    }

    Some(amplihack_home)
}

/// Roots that a *relative* recipe path may be resolved against, in priority order.
///
/// This is used when a user passes something like
/// `amplifier-bundle/recipes/quality-audit-cycle.yaml` from a directory that is
/// not the amplihack-rs repo. The resolver tries each root in order and returns
/// the first one where the joined path actually exists on disk.
///
/// Order:
///   1. cwd (preserves prior bare-name CWD-first semantics)
///   2. working_dir (the recipe runner's stated working directory)
///   3. repo root discovered by walking up from working_dir
///   4. AMPLIHACK_HOME (the installed bundle root)
///   5. ~/.amplihack
fn relative_recipe_path_search_roots(cwd: &Path, working_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_path(&mut roots, cwd.to_path_buf());
    push_unique_path(&mut roots, working_dir.to_path_buf());
    if let Some(repo_root) = repo_root_from(working_dir) {
        push_unique_path(&mut roots, repo_root);
    }
    if let Some(amplihack_home) = amplihack_home_root() {
        push_unique_path(&mut roots, amplihack_home);
    }
    if let Ok(home) = super::home_dir() {
        push_unique_path(&mut roots, home.join(".amplihack"));
    }
    roots
}

pub(crate) fn recipe_search_dirs(
    recipe_dir: Option<&str>,
    base_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let base_dir = resolve_path_from(&cwd, base_dir)?;

    if let Some(dir) = recipe_dir {
        return Ok(vec![resolve_path_from(&base_dir, dir)?]);
    }

    let mut dirs = Vec::new();
    if let Some(repo_root) = repo_root_from(&base_dir) {
        push_unique_path(
            &mut dirs,
            repo_root.join("amplifier-bundle").join("recipes"),
        );
    }

    if let Some(amplihack_home_dir) = amplihack_home_recipe_dir() {
        push_unique_path(&mut dirs, amplihack_home_dir);
    }

    push_unique_path(
        &mut dirs,
        super::home_dir()?
            .join(".amplihack")
            .join(".claude")
            .join("recipes"),
    );
    push_unique_path(&mut dirs, base_dir.join("amplifier-bundle").join("recipes"));
    push_unique_path(
        &mut dirs,
        base_dir
            .join("src")
            .join("amplihack")
            .join("amplifier-bundle")
            .join("recipes"),
    );
    push_unique_path(&mut dirs, base_dir.join(".claude").join("recipes"));

    Ok(dirs)
}

pub(crate) fn repo_root_from(base_dir: &Path) -> Option<PathBuf> {
    let mut current = base_dir.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub(crate) fn find_recipe_path(name: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    for search_dir in search_dirs {
        for extension in RECIPE_FILE_EXTENSIONS {
            let candidate = search_dir.join(format!("{name}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn find_compatible_recipe_path(name: &str, search_dirs: &[PathBuf]) -> Result<Option<PathBuf>> {
    if !is_smart_orchestrator_input(name) {
        return Ok(find_recipe_path(name, search_dirs));
    }

    let mut stale_candidates = Vec::new();
    for search_dir in search_dirs {
        for extension in RECIPE_FILE_EXTENSIONS {
            let candidate = search_dir.join(format!("{name}.{extension}"));
            if !candidate.is_file() {
                continue;
            }
            if is_incompatible_smart_orchestrator(&candidate)? {
                stale_candidates.push(candidate);
                continue;
            }
            warn_if_stale_smart_orchestrator_was_bypassed(&stale_candidates);
            return Ok(Some(candidate));
        }
    }

    fail_if_only_stale_smart_orchestrators(name, &stale_candidates)?;
    Ok(None)
}

fn is_smart_orchestrator_input(input: &str) -> bool {
    Path::new(input)
        .file_stem()
        .and_then(|value| value.to_str())
        == Some("smart-orchestrator")
}

fn warn_if_stale_smart_orchestrator_was_bypassed(stale_candidates: &[PathBuf]) {
    if stale_candidates.is_empty() {
        return;
    }

    tracing::warn!(
        stale_candidates = %stale_candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        "skipping stale smart-orchestrator recipe candidates because a compatible fallback exists"
    );
}

fn fail_if_only_stale_smart_orchestrators(input: &str, stale_candidates: &[PathBuf]) -> Result<()> {
    if stale_candidates.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "Recipe {input} resolved only to stale or incompatible smart-orchestrator candidates: {}. \
         These recipes contain legacy markers such as orch_helper/importlib/parse-decomposition. \
         Run `amplihack install` to refresh installed assets, or `amplihack update` to install a current bundle.",
        stale_candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn looks_like_recipe_path(input: &str) -> bool {
    let candidate = Path::new(input);
    candidate.components().count() > 1
        || candidate
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| RECIPE_FILE_EXTENSIONS.contains(&value))
}

pub(crate) fn resolve_recipe_path(input: &str, working_dir: impl AsRef<Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let working_dir = resolve_path_from(&cwd, working_dir)?;
    let candidate = Path::new(input);

    if candidate.is_absolute() {
        if is_smart_orchestrator_input(input)
            && candidate.is_file()
            && is_incompatible_smart_orchestrator(candidate)?
        {
            fail_if_only_stale_smart_orchestrators(input, &[candidate.to_path_buf()])?;
        }
        return Ok(candidate.to_path_buf());
    }

    if looks_like_recipe_path(input) {
        // For relative path-like inputs (e.g. `amplifier-bundle/recipes/foo.yaml`
        // or a bare `foo.yaml`), try each known root in order and return the
        // first that actually exists. This prevents the runner from claiming
        // a recipe is missing when it lives under AMPLIHACK_HOME or a parent
        // repo root rather than the current working directory.
        let roots = relative_recipe_path_search_roots(&cwd, &working_dir);
        let mut tried = Vec::with_capacity(roots.len());
        let mut stale_candidates = Vec::new();
        for root in &roots {
            let resolved = root.join(candidate);
            if resolved.is_file() {
                if is_smart_orchestrator_input(input)
                    && is_incompatible_smart_orchestrator(&resolved)?
                {
                    stale_candidates.push(resolved);
                    continue;
                }
                warn_if_stale_smart_orchestrator_was_bypassed(&stale_candidates);
                return Ok(resolved);
            }
            tried.push(resolved);
        }
        fail_if_only_stale_smart_orchestrators(input, &stale_candidates)?;

        // Preserve prior fallback behavior: if no root contains the file, return
        // the working_dir-relative path so downstream parsing emits the
        // existing "could not open recipe file" error. The caller then sees a
        // path it can reason about (the one it requested), not an arbitrary
        // search-root path. Tracing carries the full search list for debugging.
        tracing::debug!(
            input = input,
            tried = %tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
            "resolve_recipe_path: no candidate root contained a file matching input; returning working_dir-relative path"
        );
        return resolve_path_from(&working_dir, candidate);
    }

    let search_dirs = recipe_search_dirs(None, &working_dir)?;
    if let Some(resolved) = find_compatible_recipe_path(input, &search_dirs)? {
        return Ok(resolved);
    }

    anyhow::bail!(
        "Recipe not found by name: {input}. Searched: {}",
        search_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Issue #2528: where `recipe run` loaded its recipe file from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecipeOrigin {
    /// A bare name, found in the selected bundle's `recipes/`.
    Bundle,
    /// A path the user gave; sub-recipes and tools still come from the
    /// selected bundle (D6).
    File(PathBuf),
}

/// True when `recipe run` treats `input` as a path rather than a bare name:
/// it contains `/`, `\`, `..` or NUL, or ends in `.yaml` or `.yml`.
pub(crate) fn is_path_like_recipe(input: &str) -> bool {
    input.contains(['/', '\\', '\0'])
        || input.contains("..")
        || RECIPE_FILE_EXTENSIONS
            .iter()
            .any(|ext| input.ends_with(&format!(".{ext}")))
}

/// Issue #2528 (D6, E9): resolves the `recipe run` argument against one
/// bundle. A bare name is looked up only in `selection.recipes_dir()`, and
/// its canonical result must stay inside that directory. A path-like
/// argument loads from the given path, with the usual relative-path roots.
pub(crate) fn resolve_recipe_in_bundle(
    recipe: &str,
    selection: &super::run::bundle_pin::Selection,
    working_dir: &Path,
) -> Result<(PathBuf, RecipeOrigin)> {
    if is_path_like_recipe(recipe) {
        anyhow::ensure!(
            !recipe.contains('\0'),
            "recipe path {recipe:?} contains a NUL byte"
        );
        let path = resolve_recipe_path(recipe, working_dir)?;
        return Ok((path.clone(), RecipeOrigin::File(path)));
    }
    let recipes = selection.recipes_dir();
    for extension in RECIPE_FILE_EXTENSIONS {
        let candidate = recipes.join(format!("{recipe}.{extension}"));
        if std::fs::symlink_metadata(&candidate).is_err() {
            continue;
        }
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("recipe '{recipe}' resolves outside {}", recipes.display()))?;
        anyhow::ensure!(
            canonical.starts_with(recipes) && canonical.is_file(),
            "recipe '{recipe}' resolves outside {}",
            recipes.display()
        );
        return Ok((canonical, RecipeOrigin::Bundle));
    }
    let choice = selection.choice();
    let hint = match choice {
        super::run::bundle_pin::BundleChoice::Installed => {
            "; to use this checkout's recipes, pass --bundle checkout"
        }
        super::run::bundle_pin::BundleChoice::Checkout => "",
    };
    anyhow::bail!(
        "recipe '{recipe}' not found in the {} bundle at {}{hint}",
        choice.as_str(),
        selection.source().display()
    )
}
