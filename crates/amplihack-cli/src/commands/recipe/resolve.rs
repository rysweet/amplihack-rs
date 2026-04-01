use super::*;

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

pub(super) fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
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
        return Ok(candidate.to_path_buf());
    }

    if looks_like_recipe_path(input) {
        let is_bare = candidate.components().count() == 1;
        if is_bare {
            let cwd_candidate = cwd.join(candidate);
            if cwd_candidate.is_file() {
                return Ok(cwd_candidate);
            }
        }
        return resolve_path_from(&working_dir, candidate);
    }

    let search_dirs = recipe_search_dirs(None, &working_dir)?;
    if let Some(resolved) = find_recipe_path(input, &search_dirs) {
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
