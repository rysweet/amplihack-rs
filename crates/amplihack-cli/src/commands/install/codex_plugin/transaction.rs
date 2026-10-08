//! Installation transaction orchestration and commit.
use super::*;
pub(super) fn install(source: &Path, hooks_binary: &Path) -> Result<bool> {
    let home = codex_home()?;
    let root = root()?;
    // Binary selection itself probes --version, so capture before resolution.
    path_scope::with_scope(&root, &home, || {
        let Some(binary) = selected_binary()? else {
            ensure!(
                !crate::freshness::codex_selected(),
                "selected Codex executable is missing; install Codex and retry"
            );
            return Ok(false);
        };
        path_scope::check()?;
        install_scoped(source, hooks_binary, &binary, &root, &home)
    })
}

fn install_scoped(
    source: &Path,
    hooks_binary: &Path,
    binary: &Path,
    root: &Path,
    home: &Path,
) -> Result<bool> {
    // Scope capture precedes probing and any optional-provider mutations.
    let probe = native(binary, &["plugin", "list", "--json"], home);
    path_scope::check()?;
    if probe.is_err() {
        if crate::freshness::codex_selected() {
            bail!("selected Codex requires native plugin support; update Codex and retry");
        }
        println!("  ⚠️ Optional Codex plugin skipped: native plugin support unavailable");
        return Ok(false);
    }
    path_scope::check()?;
    fs::create_dir_all(root)?;
    path_scope::check()?;
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "refusing symlink Codex staging root"
    );
    let lock = fs::File::open(root)?;
    lock.lock_exclusive()?;
    path_scope::check()?;
    fs::create_dir_all(home)?;
    path_scope::check()?;
    recover_install(root, binary, home)?;
    let prepared = prepare(root, home, binary, source, hooks_binary)?;
    path_scope::check()?;
    let result = commit(root, home, binary, &prepared);
    if let Err(error) = result {
        if let Err(recovery) = recover_install(root, binary, home) {
            return Err(anyhow::anyhow!(
                "{error:#}; Codex install recovery failed: {recovery:#}"
            ));
        }
        return Err(error);
    }
    recover_install(root, binary, home)?;
    println!(
        "  ✅ Native Codex plugin installed; review exact hooks in Codex /hooks before trusting them"
    );
    Ok(true)
}

fn commit(root: &Path, home: &Path, binary: &Path, prepared: &Prepared) -> Result<()> {
    let Prepared {
        ledger,
        package,
        market,
        marketplace_path,
        marketplace,
        original_marketplace,
        old_hooks,
        hooks,
        transaction,
        staged,
    } = prepared;
    path_scope::check()?;
    amplihack_launcher::codex_config::configure(home, false)?;
    path_scope::check()?;
    atomic_json(marketplace_path, marketplace, original_marketplace.clone())?;
    path_scope::check()?;
    if package.exists() {
        fs::rename(package, root.join("previous-package"))?;
    }
    path_scope::check()?;
    fs::rename(staged.path(), package)?;
    path_scope::check()?;
    let record = Ownership {
        schema_version: 1,
        transaction: Some(transaction.clone()),
        codex_home: home.to_path_buf(),
        package_digest: digest(package)?,
        hooks: hooks.clone(),
    };
    native(
        binary,
        &[
            "plugin",
            "marketplace",
            "add",
            market
                .to_str()
                .context("Codex marketplace path must be UTF-8")?,
        ],
        home,
    )?;
    native(binary, &["plugin", "add", ID], home)?;
    let registered = native(binary, &["plugin", "list", "--json"], home)?;
    verify_identity(&registered, package)?;
    ensure!(
        installed(&registered),
        "native Codex registration missing installed identity"
    );
    reconcile_hooks(home, old_hooks, hooks)?;
    storage::sync_dependencies(root, home)?;
    let original = regular_json(ledger)?;
    atomic_json(ledger, &serde_json::to_value(&record)?, original)?;
    Ok(())
}
