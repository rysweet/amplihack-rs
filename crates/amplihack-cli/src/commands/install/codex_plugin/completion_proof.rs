//! Versioned evidence derived only from a validated completed transaction.
use super::*;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct Completion {
    schema_version: u32,
    generation: u64,
    transaction: String,
    codex_home: PathBuf,
    package_digest: String,
    declarations_sha256: String,
    snapshots: BTreeMap<String, Option<String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Base {
    schema_version: u32,
    generation: u64,
    transaction: Option<String>,
    ledger_sha256: Option<String>,
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn proof(owner: &Ownership) -> Result<&Completion> {
    let p = owner
        .completion
        .as_ref()
        .context("missing Codex completion evidence; record retained")?;
    ensure!(
        p.schema_version == 1
            && p.generation > 0
            && Some(p.transaction.as_str()) == owner.transaction.as_deref()
            && !p.transaction.is_empty()
            && p.codex_home == owner.codex_home
            && p.package_digest == owner.package_digest
            && p.declarations_sha256 == hash(&serde_json::to_vec(&owner.hooks)?)
            && p.snapshots.len() == 3
            && ["config", "hooks", "marketplace"].iter().all(|key| p
                .snapshots
                .get(*key)
                .is_some_and(|h| h.as_deref().is_some_and(valid_hash))),
        "invalid or unsupported Codex completion evidence; record retained"
    );
    Ok(p)
}
pub(super) fn base_evidence(ledger: &Option<Value>, bytes: &Value) -> Result<Value> {
    let owner = ledger
        .clone()
        .map(serde_json::from_value::<Ownership>)
        .transpose()?;
    let generation = match owner.as_ref().and_then(|o| o.completion.as_ref()) {
        Some(_) => proof(owner.as_ref().context("Codex ownership missing")?)?.generation,
        None => 0,
    };
    generation
        .checked_add(1)
        .context("Codex completion generation exhausted; record retained")?;
    let bytes: Option<Vec<u8>> = serde_json::from_value(bytes.clone())?;
    ensure!(
        bytes
            .as_ref()
            .map(|b| serde_json::from_slice::<Value>(b))
            .transpose()?
            == *ledger,
        "Codex base ownership bytes changed; record retained"
    );
    Ok(serde_json::to_value(Base {
        schema_version: 1,
        generation,
        transaction: owner.and_then(|o| o.transaction),
        ledger_sha256: bytes.as_deref().map(hash),
    })?)
}
fn base(pending: &Value) -> Result<Base> {
    let b: Base = serde_json::from_value(pending["recovery_evidence"].clone())?;
    let expected = base_evidence(
        &(!pending["ledger"].is_null()).then(|| pending["ledger"].clone()),
        &pending["snapshots"]["ledger"],
    )?;
    ensure!(
        b.schema_version == 1 && serde_json::to_value(&b)? == expected,
        "inconsistent Codex base completion evidence; record retained"
    );
    Ok(b)
}
fn snapshots(root: &Path, home: &Path) -> Result<BTreeMap<String, Option<String>>> {
    recovery_paths(root, home)
        .into_iter()
        .filter(|(key, _)| *key != "ledger")
        .map(|(key, path)| Ok((key.into(), snapshot(&path)?.as_deref().map(hash))))
        .collect()
}
/// Check independent ownership, registrations and exact completed bytes.
pub(super) fn verify(root: &Path, home: &Path, binary: &Path, owner: &Ownership) -> Result<()> {
    let p = proof(owner)?;
    ensure!(
        owner.schema_version == 1
            && owner.codex_home == home
            && !owner.uninstall_pending
            && owner.uninstall_inventory.is_none()
            && resources::current_digest(&owner.package_digest),
        "invalid completed Codex ownership; record retained"
    );
    let package = root.join("market/plugin");
    ensure!(
        digest(&package)? == owner.package_digest && declarations(&package)? == owner.hooks,
        "completed Codex package or declarations changed; record retained"
    );
    ensure!(
        snapshots(root, home)? == p.snapshots,
        "completed Codex snapshots changed; record retained"
    );
    registrations(root, home, binary)?;
    ensure!(
        snapshots(root, home)? == p.snapshots,
        "completed Codex snapshots changed; record retained"
    );
    Ok(())
}
fn registrations(root: &Path, home: &Path, binary: &Path) -> Result<()> {
    let bytes = snapshot(&home.join("config.toml"))?.context("completed Codex config missing")?;
    let doc: toml_edit::DocumentMut = std::str::from_utf8(&bytes)?.parse()?;
    let market = doc
        .get("marketplaces")
        .and_then(|m| m.get("amplihack-local"));
    let plugin = doc.get("plugins").and_then(|p| p.get(ID));
    ensure!(
        market
            .and_then(|m| m.get("source_type"))
            .and_then(toml_edit::Item::as_str)
            == Some("local")
            && market
                .and_then(|m| m.get("source"))
                .and_then(toml_edit::Item::as_str)
                == root.join("market").to_str()
            && plugin
                .and_then(|p| p.get("enabled"))
                .and_then(toml_edit::Item::as_bool)
                == Some(true),
        "completed Codex configuration registration changed; record retained"
    );
    let inventory = native(binary, &["plugin", "list", "--json"], home)?;
    verify_identity(&inventory, &root.join("market/plugin"))?;
    ensure!(
        installed(&inventory),
        "completed Codex registration missing; record retained"
    );
    Ok(())
}
pub(super) fn newer(pending: &Value, owner: &Ownership) -> Result<bool> {
    let b = base(pending)?;
    let prior: Ownership = serde_json::from_value(pending["ledger"].clone())?;
    ensure!(
        proof(&prior)?.generation == b.generation,
        "legacy base completion; record retained"
    );
    let p = proof(owner)?;
    Ok(p.generation > b.generation
        && owner.transaction != prior.transaction
        && owner.transaction.as_deref() != pending["transaction"].as_str())
}
/// Durably anticipate the exact completed ledger before ownership publication.
pub(super) fn complete(
    root: &Path,
    home: &Path,
    binary: &Path,
    owner: &mut Ownership,
) -> Result<()> {
    let path = root.join("pending.json");
    let mut pending = regular_json(&path)?.context("Codex completion journal missing")?;
    recovery::validate_journal(&pending, home)?;
    preflight(&pending, root, home)?;
    ensure!(
        owner.transaction.as_deref() == pending["transaction"].as_str()
            && Some(owner.package_digest.as_str()) == pending["target_digest"].as_str()
            && owner.hooks == pending["target_hooks"],
        "Codex planned completion changed; record retained"
    );
    for (key, path) in recovery_paths(root, home)
        .into_iter()
        .filter(|(k, _)| ["hooks", "marketplace"].contains(k))
    {
        ensure!(
            serde_json::to_value(snapshot(&path)?)? == pending["expected"][key],
            "foreign Codex {key} changed before completion; record retained"
        );
    }
    registrations(root, home, binary)?;
    preflight(&pending, root, home)?;
    let generation = base(&pending)?
        .generation
        .checked_add(1)
        .context("Codex completion generation exhausted; record retained")?;
    owner.completion = Some(Completion {
        schema_version: 1,
        generation,
        transaction: owner
            .transaction
            .clone()
            .context("Codex transaction missing")?,
        codex_home: home.into(),
        package_digest: owner.package_digest.clone(),
        declarations_sha256: hash(&serde_json::to_vec(&owner.hooks)?),
        snapshots: snapshots(root, home)?,
    });
    verify(root, home, binary, owner)?;
    storage::sync_dependencies(root, home)?;
    let original = pending.clone();
    pending["expected"]["ledger"] = json!(json_bytes(&serde_json::to_value(&*owner)?)?);
    atomic_json(&path, &pending, Some(original))?;
    preflight(&pending, root, home)?;
    verify(root, home, binary, owner)
}
