use std::path::PathBuf;

pub(super) fn find_recipe_runner_binary() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var("RECIPE_RUNNER_RS_PATH")
        && let Some(resolved) = resolve_binary_path(&path)
    {
        return Ok(resolved);
    }

    for candidate in [
        "recipe-runner-rs",
        "~/.cargo/bin/recipe-runner-rs",
        "~/.local/bin/recipe-runner-rs",
    ] {
        if let Some(resolved) = resolve_binary_path(candidate) {
            return Ok(resolved);
        }
    }

    anyhow::bail!(
        "recipe-runner-rs binary not found. Install it: cargo install --git https://github.com/rysweet/amplihack-recipe-runner or set RECIPE_RUNNER_RS_PATH."
    )
}

pub(super) fn resolve_binary_path(candidate: &str) -> Option<PathBuf> {
    let expanded = if let Some(rest) = candidate.strip_prefix("~/") {
        home_dir().ok()?.join(rest)
    } else {
        PathBuf::from(candidate)
    };

    if expanded.components().count() > 1 {
        return expanded.is_file().then_some(expanded);
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&expanded))
        .find(|entry| entry.is_file())
}

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("HOME not set"))
}
