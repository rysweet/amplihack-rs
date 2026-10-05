use std::path::PathBuf;

pub(super) fn find_recipe_runner_binary() -> anyhow::Result<PathBuf> {
    crate::freshness::probe_recipe_runner()?;
    crate::rust_toolchain::find_recipe_runner().ok_or_else(|| {
        anyhow::anyhow!(
            "recipe-runner-rs binary not found. Install it: cargo install --git https://github.com/rysweet/amplihack-recipe-runner or set RECIPE_RUNNER_RS_PATH."
        )
    })
}
