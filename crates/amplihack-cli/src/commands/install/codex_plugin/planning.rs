//! Read-only transaction validation and journal planning.
use super::*;
pub(super) struct Prepared {
    pub(super) ledger: PathBuf,
    pub(super) package: PathBuf,
    pub(super) market: PathBuf,
    pub(super) marketplace_path: PathBuf,
    pub(super) marketplace: Value,
    pub(super) original_marketplace: Option<Value>,
    pub(super) old_hooks: Value,
    pub(super) hooks: Value,
    pub(super) transaction: String,
    pub(super) staged: tempfile::TempDir,
}
pub(super) fn prepare(
    root: &Path,
    home: &Path,
    binary: &Path,
    source: &Path,
    hooks_binary: &Path,
) -> Result<Prepared> {
    match fs::symlink_metadata(root.join("previous-package")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
        Ok(_) => bail!("unowned Codex package backup exists; preserve it and reconcile manually"),
    }
    let ledger = root.join("ownership.json");
    let previous = regular_json(&ledger)?
        .map(serde_json::from_value::<Ownership>)
        .transpose()?;
    if let Some(old) = &previous {
        ensure!(
            old.schema_version == 1
                && old.codex_home == home
                && resources::current_digest(&old.package_digest),
            "legacy or changed Codex ownership proof; preserve package and reconcile manually"
        );
    }
    let package = root.join("market/plugin");
    let inventory = native(binary, &["plugin", "list", "--json"], home)?;
    verify_identity(&inventory, &package)?;
    ensure!(
        previous.is_some() || !installed(&inventory),
        "unowned existing Codex identity; refusing replacement"
    );
    if package.exists() {
        ensure!(
            !fs::symlink_metadata(&package)?.file_type().is_symlink(),
            "refusing symlink Codex package"
        );
        let old = previous
            .as_ref()
            .context("unowned Codex package; refusing replacement")?;
        ensure!(
            digest(&package)? == old.package_digest,
            "Codex package changed outside installer; preserve it and reconcile manually"
        );
    }
    path_scope::check()?;
    let staged = tempfile::tempdir_in(root)?;
    build(source, hooks_binary, staged.path())?;
    let hooks = declarations(&package)?;
    let market = root.join("market");
    path_scope::check()?;
    fs::create_dir_all(market.join(".agents/plugins"))?;
    path_scope::check()?;
    let marketplace_path = market.join(".agents/plugins/marketplace.json");
    let original_marketplace = regular_json(&marketplace_path)?;
    let mut marketplace = original_marketplace
        .clone()
        .unwrap_or_else(|| json!({"name":"amplihack-local","plugins":[]}));
    ensure!(
        marketplace["name"] == "amplihack-local",
        "foreign marketplace identity; refusing replacement"
    );
    let entries = marketplace["plugins"]
        .as_array_mut()
        .context("marketplace plugins must be an array")?;
    let expected_entry = json!({"name":"amplihack", "source":{"source":"local","path":"./plugin"},
        "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"}, "category":"Productivity"});
    ensure!(
        entries.iter().all(|entry| entry["name"] != "amplihack"
            || (previous.is_some() && entry == &expected_entry)),
        "foreign local marketplace entry; refusing replacement"
    );
    entries.retain(|entry| entry["name"] != "amplihack");
    entries.push(json!({"name":"amplihack", "source":{"source":"local","path":"./plugin"},
        "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"}, "category":"Productivity"}));
    let old_hooks = previous
        .as_ref()
        .map(|p| p.hooks.clone())
        .unwrap_or_else(|| json!({}));
    let original_hooks = regular_json(&home.join("hooks.json"))?;
    let expected_hooks = merged_hooks(original_hooks.clone(), &old_hooks, &hooks)?;
    let original_ledger = regular_json(&ledger)?;
    let config = home.join("config.toml");
    let original_config = match fs::symlink_metadata(&config) {
        Ok(m) => {
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "refusing nonregular Codex config"
            );
            Some(fs::read(&config)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let config_states =
        config::config_states(original_config.as_deref(), &market, previous.is_some())?;
    let transaction = staged
        .path()
        .file_name()
        .context("staging name missing")?
        .to_string_lossy()
        .into_owned();
    let snapshots = json!({"hooks":snapshot(&home.join("hooks.json"))?, "marketplace":snapshot(&marketplace_path)?, "ledger":snapshot(&ledger)?});
    let target_digest = digest(staged.path())?;
    let record = Ownership {
        schema_version: 1,
        transaction: Some(transaction.clone()),
        codex_home: home.to_path_buf(),
        package_digest: target_digest.clone(),
        hooks: hooks.clone(),
    };
    let expected = json!({"config":original_config,"hooks":json_bytes(&expected_hooks)?,
        "marketplace":json_bytes(&marketplace)?,"ledger":json_bytes(&serde_json::to_value(record)?)?});
    let pending = json!({"config_states":config_states,"expected":expected,"snapshots":snapshots,"schema_version":2,"codex_home":home,"transaction":transaction,"config":original_config,"marketplace":original_marketplace, "hooks":original_hooks,
        "ledger":original_ledger, "installed":installed(&inventory), "had_package":package.exists(), "target_digest":target_digest, "target_hooks":hooks});
    atomic_json(&root.join("pending.json"), &pending, None)?;
    // atomic_json checks the journal file and directory durability before mutation.
    Ok(Prepared {
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
    })
}
