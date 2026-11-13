//! Invocation-bound drive policy from the authenticated target base tree.
use super::{
    authorization::InvocationAuthorization, hosting::*, records::Binding, state::EvidenceStore,
    validation,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveMode {
    Merge,
    Handoff,
}
impl std::str::FromStr for DriveMode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "merge" => Ok(Self::Merge),
            "handoff" => Ok(Self::Handoff),
            _ => bail!("drive-mode-invalid: merge must be exactly merge or handoff"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPolicy {
    pub schema_version: u32,
    pub binding: Binding,
    pub mode: DriveMode,
    pub source: String,
    pub configuration_sha256: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    schema_version: u32,
    run_id: String,
    host: String,
    repository: String,
    pr_number: u64,
    pr_url: String,
}
fn selection(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    b: &Binding,
    establish: bool,
) -> Result<()> {
    let name = format!("selection-{}.json", auth.run_id());
    let expected = Selection {
        schema_version: 1,
        run_id: auth.run_id().into(),
        host: b.host.clone(),
        repository: b.repository.clone(),
        pr_number: b.pr_number,
        pr_url: b.pr_url.clone(),
    };
    match store.read_optional(&name)? {
        Some(bytes) => ensure!(
            serde_json::from_slice::<Selection>(&bytes)? == expected,
            "drive-selection-changed: another PR requires a new invocation"
        ),
        None if establish => store.write_new(&name, &serde_json::to_vec(&expected)?)?,
        None => bail!("drive-selection-missing: authenticated PR selection is required"),
    }
    Ok(())
}

/// Round labels change during review; all identity, commit and policy fields remain bound.
pub(crate) fn same_binding(a: &Binding, b: &Binding) -> bool {
    let mut a = a.clone();
    a.round_id.clone_from(&b.round_id);
    a == *b
}
fn name(binding: &Binding) -> Result<String> {
    let mut key = binding.clone();
    key.round_id.clear();
    Ok(format!(
        "policy-{:x}.json",
        Sha256::digest(serde_json::to_vec(&key)?)
    ))
}
fn check(auth: &InvocationAuthorization, b: &Binding) -> Result<()> {
    validation::validate_binding(b)?;
    auth.validate_selection(b)?;
    ensure!(
        b.run_id == auth.run_id() && auth.policy_provenance() == Some(b.policy_provenance.as_str()),
        "drive-policy-binding: policy belongs to another invocation or launch bundle"
    );
    Ok(())
}
pub fn resolve(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    binding: &Binding,
    host: &mut impl PolicyHosting,
) -> Result<ResolvedPolicy> {
    check(auth, binding)?;
    store.require_bound(auth)?;
    let observed = host
        .observe_pr(binding)
        .context("drive-policy-observe: cannot establish target PR")?;
    ensure!(
        same_binding(binding, &observed.binding),
        "drive-policy-stale: observed PR/head/base changed"
    );
    selection(auth, store, binding, true)?;
    let tip = host
        .base_tip(binding)
        .context("drive-policy-base: cannot establish authoritative base")?;
    ensure!(
        tip == binding.base_sha,
        "drive-policy-base-changed: resolve again against the current base"
    );
    let mut policy = ResolvedPolicy {
        schema_version: 1,
        binding: binding.clone(),
        mode: DriveMode::Merge,
        source: "default".into(),
        configuration_sha256: None,
    };
    if let Some(mode) = auth.explicit_mode() {
        policy.mode = mode;
        policy.source = "explicit".into();
    } else {
        match host
            .read_base_config(binding, &tip, ".amplihack/auto-drive.toml")
            .context("drive-policy-config: cannot read authoritative base configuration")?
        {
            BaseConfig::Missing => (),
            BaseConfig::File { bytes, mode } => {
                ensure!(
                    matches!(mode.as_str(), "100644" | "100755"),
                    "drive-policy-entry: configuration must be a regular blob"
                );
                ensure!(
                    bytes.len() <= validation::MAX_RECORD_BYTES,
                    "drive-policy-size: configuration exceeds size limit"
                );
                let text = std::str::from_utf8(&bytes)
                    .context("drive-policy-encoding: configuration must be UTF-8")?;
                let config: toml::Table =
                    toml::from_str(text).map_err(|error: toml::de::Error| {
                        anyhow::anyhow!(
                            "drive-policy-toml: malformed base configuration near byte {}",
                            error.span().map_or(0, |span| span.start)
                        )
                    })?;
                policy.configuration_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
                if let Some(value) = config.get("merge") {
                    policy.mode = value
                        .as_str()
                        .context("drive-policy-type: merge must be a string")?
                        .parse()?;
                    policy.source = "base_configuration".into();
                }
            }
        }
    }
    // Re-resolution may verify an identical immutable record, never replace one.
    let filename = name(binding)?;
    match store.read_optional(&filename)? {
        Some(bytes) => {
            let previous: ResolvedPolicy = serde_json::from_slice(&bytes)?;
            policy
                .binding
                .round_id
                .clone_from(&previous.binding.round_id);
            ensure!(
                previous == policy,
                "drive-policy-conflict: persisted policy differs"
            );
        }
        None => store.write_new(&filename, &serde_json::to_vec(&policy)?)?,
    }
    Ok(policy)
}
pub(crate) fn load(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    binding: &Binding,
) -> Result<ResolvedPolicy> {
    check(auth, binding)?;
    store.require_bound(auth)?;
    selection(auth, store, binding, false)?;
    let policy: ResolvedPolicy = serde_json::from_slice(
        &store
            .read_private(&name(binding)?)
            .context("drive-policy-missing: resolve policy before completion")?,
    )?;
    ensure!(
        policy.schema_version == 1 && same_binding(&policy.binding, binding),
        "drive-policy-binding: stale policy"
    );
    ensure!(
        policy
            .configuration_sha256
            .as_ref()
            .is_none_or(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())),
        "drive-policy-digest-invalid"
    );
    match (auth.explicit_mode(), policy.source.as_str()) {
        (Some(mode), "explicit") => ensure!(
            mode == policy.mode && policy.configuration_sha256.is_none(),
            "drive-policy-mode: captured selection differs"
        ),
        (None, "default") => ensure!(
            policy.mode == DriveMode::Merge,
            "drive-policy-default-invalid"
        ),
        (None, "base_configuration") => ensure!(
            policy.configuration_sha256.is_some(),
            "drive-policy-configuration-missing"
        ),
        _ => bail!("drive-policy-source: incompatible policy authority"),
    }
    Ok(policy)
}
pub(crate) fn require(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    policy: &ResolvedPolicy,
    mode: DriveMode,
) -> Result<()> {
    ensure!(
        auth.allows_drive(),
        "drive-action-restricted: entrypoint or dry-run cannot complete a drive action"
    );
    let stored = load(auth, store, &policy.binding)?;
    ensure!(
        stored == *policy && stored.mode == mode,
        "drive-policy-mode: action differs from durable policy"
    );
    Ok(())
}
