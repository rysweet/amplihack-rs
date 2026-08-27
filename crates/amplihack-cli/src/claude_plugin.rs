//! Claude Code plugin registration and canonical skill staging for amplihack.
//!
//! Canonical skills are installed once as direct children of
//! `~/.claude/skills`. The `amplihack` plugin wrapper in that directory
//! retains non-skill assets only, avoiding duplicate skill discovery.
//!
//! Earlier versions of this module staged into `~/.claude/plugins/amplihack/`
//! and hand-wrote an `enabledPlugins` entry to
//! `~/.config/claude-code/plugins.json`. Neither of those matched what the
//! Claude Code binary actually reads (`claude plugin list` reported no
//! plugins installed despite that file existing, and `claude plugin
//! validate` separately rejected the manifest's `author` field being a
//! bare string instead of an object), so the plugin was silently never
//! discovered. The skills-dir mechanism sidesteps all of that: no
//! undocumented state file to keep in sync, no network/marketplace
//! resolution, and it's verified against `claude plugin validate`.
//!
//! The plugin directory is built from the staged framework under
//! `~/.amplihack/.claude/` (populated by `amplihack install`). We use
//! symlinks when possible so subsequent framework updates are picked up
//! automatically, and fall back to copies when symlinks fail (e.g. on
//! Windows without developer mode, or across filesystem boundaries).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const PLUGIN_NAME: &str = "amplihack";
const PLUGIN_VERSION: &str = crate::VERSION;
const SKILL_OWNERSHIP_VERSION: u32 = 2;
const SKILL_OWNERSHIP_MANIFEST: &str = "claude-skills-manifest.json";

/// Top-level plugin assets mirrored from the staged framework dir into the
/// Claude Code plugin dir. Only asset types Claude Code discovers need to
/// be listed here.
const MIRRORED_ASSETS: &[&str] = &["agents", "commands", "context", "workflow"];

struct KnownSkillLink {
    path: &'static str,
    link_target: &'static str,
    canonical_target: &'static str,
}

const KNOWN_SKILL_LINKS: &[KnownSkillLink] = &[
    KnownSkillLink {
        path: "docx/ooxml",
        link_target: "../common/ooxml",
        canonical_target: "common/ooxml",
    },
    KnownSkillLink {
        path: "pptx/ooxml",
        link_target: "../common/ooxml",
        canonical_target: "common/ooxml",
    },
    KnownSkillLink {
        path: "outside-in-testing/README.md",
        link_target: "../qa-team/README.md",
        canonical_target: "qa-team/README.md",
    },
    KnownSkillLink {
        path: "outside-in-testing/examples",
        link_target: "../qa-team/examples",
        canonical_target: "qa-team/examples",
    },
    KnownSkillLink {
        path: "outside-in-testing/scripts",
        link_target: "../qa-team/scripts",
        canonical_target: "qa-team/scripts",
    },
    KnownSkillLink {
        path: "outside-in-testing/tests",
        link_target: "../qa-team/tests",
        canonical_target: "qa-team/tests",
    },
];

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SkillOwnershipManifest {
    version: u32,
    skills: Vec<OwnedDestination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<OwnedDestination>,
    /// Ownership of `~/.claude/commands/amplihack/` (issue #1344).
    ///
    /// Optional and defaulted so a manifest written before the slash commands
    /// were staged still parses under `deny_unknown_fields`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commands: Option<OwnedDestination>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedDestination {
    name: String,
    content_sha256: String,
}

/// Ensure amplihack is staged as a Claude Code skills-dir plugin.
///
/// Idempotent: safe to call on every launcher start. Returns install
/// errors to the caller; callers should treat failures as non-fatal so a
/// plugin issue does not block Claude launch.
pub fn ensure_claude_plugin_installed() -> Result<()> {
    let staged = staged_framework_dir()?;
    if !staged.is_dir() {
        tracing::debug!(
            path = %staged.display(),
            "staged amplihack framework not found; skipping Claude plugin install"
        );
        return Ok(());
    }
    let plugin_dir = plugin_install_dir()?;
    let ownership_path = skill_ownership_manifest_path(&staged);
    let ownership = read_skill_ownership_manifest(&ownership_path)?;
    validate_plugin_destination(&staged, &plugin_dir, ownership.plugin.as_ref())?;

    sync_canonical_skills(
        &staged.join("skills"),
        plugin_dir
            .parent()
            .context("Claude plugin directory has no skills parent")?,
        &ownership_path,
    )?;
    publish_plugin_wrapper(&staged, &plugin_dir, &ownership_path)
}

fn validate_plugin_destination(
    staged: &Path,
    plugin_dir: &Path,
    owned: Option<&OwnedDestination>,
) -> Result<()> {
    match fs::symlink_metadata(plugin_dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", plugin_dir.display()));
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => bail!(
            "Claude plugin wrapper has unsupported type: {}",
            plugin_dir.display()
        ),
    }
    if owned.is_none() {
        return validate_legacy_plugin_wrapper(staged, plugin_dir);
    }
    let owned = owned.with_context(|| {
        format!(
            "Claude plugin wrapper conflicts with unowned destination {}",
            plugin_dir.display()
        )
    })?;
    if hash_directory_tree(plugin_dir)? != owned.content_sha256 {
        bail!(
            "Claude plugin wrapper no longer matches amplihack ownership state at {}; preserving replaced content",
            plugin_dir.display()
        );
    }
    Ok(())
}

fn validate_legacy_plugin_wrapper(staged: &Path, plugin_dir: &Path) -> Result<()> {
    let entries = fs::read_dir(plugin_dir)
        .with_context(|| format!("failed to read legacy wrapper {}", plugin_dir.display()))?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("legacy Claude plugin wrapper has a non-UTF-8 entry"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut expected_entries = BTreeSet::from([".claude-plugin".to_owned()]);
    for asset in ["agents", "skills", "commands", "context", "workflow"] {
        if resolve_asset_source(staged, asset).is_dir() {
            expected_entries.insert(asset.to_owned());
        }
    }
    if !expected_entries.contains("skills") || entries != expected_entries {
        bail!(
            "Claude plugin wrapper conflicts with unowned destination {}",
            plugin_dir.display()
        );
    }

    let manifest_dir = plugin_dir.join(".claude-plugin");
    let manifest_dir_metadata = fs::symlink_metadata(&manifest_dir)
        .with_context(|| format!("failed to inspect {}", manifest_dir.display()))?;
    if !manifest_dir_metadata.is_dir() || manifest_dir_metadata.file_type().is_symlink() {
        bail!(
            "Claude plugin wrapper conflicts with unowned destination {}",
            plugin_dir.display()
        );
    }
    let manifest_entries = fs::read_dir(&manifest_dir)
        .with_context(|| format!("failed to read {}", manifest_dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    if manifest_entries.len() != 1
        || manifest_entries[0].file_name() != "plugin.json"
        || !manifest_entries[0].file_type()?.is_file()
    {
        bail!(
            "Claude plugin wrapper conflicts with unowned destination {}",
            plugin_dir.display()
        );
    }
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_dir.join("plugin.json"))
            .context("failed to read legacy wrapper manifest")?,
    )
    .context("invalid legacy Claude plugin manifest")?;
    let expected_author = json!({"name": "Microsoft"});
    if manifest.get("$schema").and_then(Value::as_str)
        != Some("https://anthropic.com/claude-code/plugin.schema.json")
        || manifest.get("name").and_then(Value::as_str) != Some(PLUGIN_NAME)
        || !manifest
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(is_legacy_plugin_version)
        || manifest.get("description").and_then(Value::as_str)
            != Some("Amplihack AI development framework — agents, skills, and commands.")
        || manifest.get("author") != Some(&expected_author)
        || manifest.get("skills") != Some(&json!(["./skills"]))
        || manifest.as_object().is_none_or(|object| object.len() != 6)
    {
        bail!(
            "Claude plugin wrapper conflicts with unowned destination {}",
            plugin_dir.display()
        );
    }

    for asset in entries
        .iter()
        .filter(|entry| entry.as_str() != ".claude-plugin")
    {
        validate_legacy_asset(
            &resolve_asset_source(staged, asset),
            &plugin_dir.join(asset),
            asset,
        )?;
    }
    Ok(())
}

fn is_legacy_plugin_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn validate_legacy_asset(source: &Path, destination: &Path, asset: &str) -> Result<()> {
    if !source.is_dir() {
        bail!(
            "legacy Claude plugin asset has no canonical source: {}",
            destination.display()
        );
    }
    let metadata = fs::symlink_metadata(destination)
        .with_context(|| format!("failed to inspect {}", destination.display()))?;
    if metadata.file_type().is_symlink() {
        let actual = fs::canonicalize(destination)
            .with_context(|| format!("failed to resolve {}", destination.display()))?;
        let expected = fs::canonicalize(source)
            .with_context(|| format!("failed to resolve {}", source.display()))?;
        if actual == expected {
            return Ok(());
        }
    } else if metadata.is_dir() {
        let matches = if asset == "skills" {
            legacy_skill_trees_match(source, destination)?
        } else {
            let expected = tempfile::tempdir().context("failed to validate legacy plugin copy")?;
            let expected_copy = expected.path().join("asset");
            copy_dir_recursive(source, &expected_copy)?;
            hash_directory_tree(destination)? == hash_directory_tree(&expected_copy)?
        };
        if matches {
            return Ok(());
        }
    }
    bail!(
        "Claude plugin wrapper conflicts with unowned destination {}",
        destination.display()
    )
}

fn legacy_skill_trees_match(source: &Path, destination: &Path) -> Result<bool> {
    if hash_skill_tree_without_known_links(source)?
        != hash_skill_tree_without_known_links(destination)?
    {
        return Ok(false);
    }

    for link in KNOWN_SKILL_LINKS {
        if !known_link_representation_matches(source, destination, link)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn hash_skill_tree_without_known_links(root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.retain(|(relative, _)| {
        !KNOWN_SKILL_LINKS.iter().any(|link| {
            let link_path = Path::new(link.path);
            relative == link_path || relative.starts_with(link_path)
        })
    });
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    hash_tree_entries(root, entries)
}

fn known_link_representation_matches(
    canonical_skills: &Path,
    legacy_skills: &Path,
    link: &KnownSkillLink,
) -> Result<bool> {
    let legacy_path = legacy_skills.join(link.path);
    let metadata = match fs::symlink_metadata(&legacy_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", legacy_path.display()));
        }
    };

    if metadata.file_type().is_symlink() {
        return Ok(fs::read_link(&legacy_path)
            .with_context(|| format!("failed to read symlink {}", legacy_path.display()))?
            == Path::new(link.link_target));
    }

    let canonical_target = canonical_skills.join(link.canonical_target);
    let canonical_metadata = match fs::metadata(&canonical_target) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", canonical_target.display()));
        }
    };

    if metadata.is_file() {
        let contents = fs::read(&legacy_path)
            .with_context(|| format!("failed to read {}", legacy_path.display()))?;
        if contents == link.link_target.as_bytes() {
            return Ok(true);
        }
        return match canonical_metadata {
            Some(target) if target.is_file() => Ok(contents == fs::read(&canonical_target)?),
            _ => Ok(false),
        };
    }

    if metadata.is_dir() {
        return match canonical_metadata {
            Some(target) if target.is_dir() => {
                Ok(hash_directory_tree(&legacy_path)? == hash_directory_tree(&canonical_target)?)
            }
            _ => Ok(false),
        };
    }

    Ok(false)
}

/// Bring `~/.claude/skills/amplihack` up to date.
///
/// `validate_plugin_destination` has already established that whatever sits
/// there is amplihack's own. When a hash-verified wrapper is present it is
/// reconciled *in place*: each piece is compared before it is touched, and any
/// replacement is published with a `rename` over the live path. A launch that
/// changes nothing therefore performs no writes and no unlinks, and no instant
/// of a launch leaves an asset tree missing (issue #1273). That is also what
/// makes concurrent launches from different directories safe — they either
/// both find the wrapper current and do nothing, or they race two renames that
/// publish the same content.
///
/// A first install, and the one-time adoption of a pre-ownership wrapper,
/// still publish through a sibling transaction directory and a single rename,
/// so a failure part-way through leaves nothing live.
fn publish_plugin_wrapper(staged: &Path, plugin_dir: &Path, ownership_path: &Path) -> Result<()> {
    let skills_root = plugin_dir
        .parent()
        .context("Claude plugin directory has no skills parent")?;
    fs::create_dir_all(skills_root)
        .with_context(|| format!("failed to create {}", skills_root.display()))?;
    let ownership = read_skill_ownership_manifest(ownership_path)?;
    if ownership.plugin.is_some() && is_real_directory(plugin_dir)? {
        return reconcile_plugin_wrapper(
            staged,
            plugin_dir,
            ownership_path,
            skills_root,
            ownership,
        );
    }
    install_plugin_wrapper(staged, plugin_dir, ownership_path, skills_root)
}

/// Refresh an existing amplihack-owned wrapper without ever taking it apart.
///
/// The ownership digest proved the wrapper is byte-for-byte what amplihack
/// last published, so every entry under it is ours to compare and, only where
/// it differs, to replace.
fn reconcile_plugin_wrapper(
    staged: &Path,
    plugin_dir: &Path,
    ownership_path: &Path,
    skills_root: &Path,
    mut ownership: SkillOwnershipManifest,
) -> Result<()> {
    let mut scratch = Scratch::new(skills_root);
    let mut changed = write_plugin_manifest(plugin_dir, &mut scratch)?;
    changed |= mirror_assets(staged, plugin_dir, &mut scratch)?;
    changed |= prune_retired_assets(plugin_dir)?;
    if !changed {
        return Ok(());
    }
    ownership.plugin = Some(OwnedDestination {
        name: PLUGIN_NAME.to_owned(),
        content_sha256: hash_directory_tree(plugin_dir)?,
    });
    write_skill_ownership_manifest(ownership_path, &ownership)
}

/// Drop wrapper entries amplihack no longer publishes.
///
/// Reached only for a hash-verified wrapper, so anything outside the current
/// asset set is a leftover from an older release — `skills` used to be
/// mirrored here — rather than something a user put there.
fn prune_retired_assets(plugin_dir: &Path) -> Result<bool> {
    let mut changed = false;
    for entry in fs::read_dir(plugin_dir)
        .with_context(|| format!("failed to read {}", plugin_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let retired = name
            .to_str()
            .is_none_or(|name| name != ".claude-plugin" && !MIRRORED_ASSETS.contains(&name));
        if retired {
            changed |= remove_mirror(&entry.path())?;
        }
    }
    Ok(changed)
}

/// Publish a wrapper where none is currently live.
///
/// The whole tree is built inside a sibling transaction directory and moved
/// into place with one rename, so an interrupted install leaves either the
/// previous state or the new one — never a half-built wrapper.
fn install_plugin_wrapper(
    staged: &Path,
    plugin_dir: &Path,
    ownership_path: &Path,
    skills_root: &Path,
) -> Result<()> {
    let transaction = tempfile::Builder::new()
        .prefix(".amplihack-plugin-transaction-")
        .tempdir_in(skills_root)
        .context("failed to create sibling plugin transaction directory")?;
    let staged_plugin = transaction.path().join("staged").join(PLUGIN_NAME);
    fs::create_dir_all(&staged_plugin)
        .with_context(|| format!("failed to create {}", staged_plugin.display()))?;
    {
        let mut scratch = Scratch::new(transaction.path());
        write_plugin_manifest(&staged_plugin, &mut scratch)?;
        mirror_assets(staged, &staged_plugin, &mut scratch)?;
    }
    let plugin_ownership = OwnedDestination {
        name: PLUGIN_NAME.to_owned(),
        content_sha256: hash_directory_tree(&staged_plugin)?,
    };

    let backup_root = transaction.path().join("backups");
    fs::create_dir_all(&backup_root)
        .with_context(|| format!("failed to create {}", backup_root.display()))?;
    let backup = move_destination_to_backup(plugin_dir, &backup_root, PLUGIN_NAME)?;
    let applied = [AppliedSkill {
        destination: plugin_dir.to_path_buf(),
        backup,
    }];
    let result = (|| -> Result<()> {
        fs::rename(&staged_plugin, plugin_dir)
            .context("failed to publish Claude plugin wrapper")?;
        let mut ownership = read_skill_ownership_manifest(ownership_path)?;
        ownership.plugin = Some(plugin_ownership);
        write_skill_ownership_manifest(ownership_path, &ownership)
    })();
    if let Err(error) = result {
        return Err(skill_transaction_error(error, &applied, transaction));
    }
    transaction
        .close()
        .context("failed to remove completed plugin transaction directory")
}

/// Remove only Claude destinations whose contents still match amplihack's
/// external ownership record. Replaced or otherwise unverified content is
/// preserved, and the ownership record is left for the normal uninstall path.
pub(crate) fn remove_managed_claude_plugin() -> Result<()> {
    let staged = staged_framework_dir()?;
    let manifest_path = skill_ownership_manifest_path(&staged);
    let ownership = read_skill_ownership_manifest(&manifest_path)?;
    let skills_root = plugin_install_dir()?
        .parent()
        .context("Claude plugin directory has no skills parent")?
        .to_path_buf();

    for skill in &ownership.skills {
        remove_verified_destination(&skills_root.join(&skill.name), skill)?;
    }
    if let Some(plugin) = &ownership.plugin {
        remove_verified_destination(&skills_root.join(PLUGIN_NAME), plugin)?;
    }
    Ok(())
}

fn remove_verified_destination(path: &Path, ownership: &OwnedDestination) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            tracing::warn!(
                path = %path.display(),
                "preserving Claude destination because its root type no longer matches amplihack ownership state"
            );
            return Ok(());
        }
    }
    let actual = hash_directory_tree(path)?;
    if actual != ownership.content_sha256 {
        tracing::warn!(
            path = %path.display(),
            "preserving Claude destination because it no longer matches amplihack ownership state"
        );
        return Ok(());
    }
    fs::remove_dir_all(path).with_context(|| {
        format!(
            "failed to remove managed Claude destination {}",
            path.display()
        )
    })
}

/// Path of the ownership manifest for a staged framework root.
///
/// Exposed so the installer and uninstaller can name the same record without
/// re-deriving the layout, and so tests can point at a temp root.
pub(crate) fn ownership_manifest_path(staged_framework: &Path) -> PathBuf {
    skill_ownership_manifest_path(staged_framework)
}

/// Whether `target` still contains exactly what amplihack last staged into
/// `~/.claude/commands/amplihack/`.
///
/// `false` — including "no record yet" and "the directory is a symlink" —
/// means the installer must treat the directory's contents as possibly the
/// user's own and preserve anything it does not itself provide.
pub(crate) fn claude_commands_are_owned(manifest_path: &Path, target: &Path) -> Result<bool> {
    let Some(owned) = read_skill_ownership_manifest(manifest_path)?.commands else {
        return Ok(false);
    };
    match fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", target.display()));
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(false),
    }
    Ok(hash_directory_tree(target)? == owned.content_sha256)
}

/// Record `target`'s current contents as amplihack-owned.
///
/// Called after the command swap completes, so uninstall can later tell an
/// untouched amplihack command set apart from a directory the user has since
/// made their own.
pub(crate) fn record_claude_commands_ownership(manifest_path: &Path, target: &Path) -> Result<()> {
    let mut ownership = read_skill_ownership_manifest(manifest_path)?;
    ownership.commands = Some(OwnedDestination {
        name: PLUGIN_NAME.to_string(),
        content_sha256: hash_directory_tree(target)?,
    });
    write_skill_ownership_manifest(manifest_path, &ownership)
}

/// Outcome of the uninstall-time removal of the Claude command namespace.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommandRemoval {
    /// Nothing was there.
    Absent,
    /// Contents matched the ownership record and were removed.
    Removed,
    /// Contents no longer match; the directory was left alone.
    Preserved,
}

/// Remove `~/.claude/commands/amplihack/` only while it still matches the
/// ownership record written at install time.
///
/// The directory sits under the user's own `~/.claude`, so an unconditional
/// `remove_dir_all` here destroys a `my-thing.md` the user put in the same
/// namespace. This is the `remove_verified_destination` contract the managed
/// plugin destinations already use: refuse non-directories and symlinks, hash
/// the tree, and preserve when the hash no longer matches.
pub(crate) fn remove_managed_claude_commands(
    manifest_path: &Path,
    target: &Path,
) -> Result<CommandRemoval> {
    if !target.exists() {
        return Ok(CommandRemoval::Absent);
    }
    let Some(owned) = read_skill_ownership_manifest(manifest_path)?.commands else {
        tracing::warn!(
            path = %target.display(),
            "preserving Claude command namespace because amplihack has no ownership record for it"
        );
        return Ok(CommandRemoval::Preserved);
    };
    remove_verified_destination(target, &owned)?;
    Ok(if target.exists() {
        CommandRemoval::Preserved
    } else {
        CommandRemoval::Removed
    })
}

fn staged_framework_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(".claude"))
}

/// Claude Code auto-loads any plugin directory dropped under
/// `~/.claude/skills/` as `<name>@skills-dir` — this is the same location
/// `claude plugin init <name>` scaffolds into.
fn plugin_install_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude").join("skills").join(PLUGIN_NAME))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

fn skill_ownership_manifest_path(staged: &Path) -> PathBuf {
    staged.join("install").join(SKILL_OWNERSHIP_MANIFEST)
}

/// Bring `.claude-plugin/plugin.json` in line with the running binary, and
/// return whether that required a write.
///
/// The manifest carries the installed amplihack version, so it genuinely has
/// to be refreshed after an upgrade — but it was previously rewritten on every
/// launch regardless, which both churned the file and made a full, destructive
/// republication of the wrapper look necessary. Compare the bytes first, and
/// when they do differ publish through a scratch file plus `rename` so a
/// concurrent reader never sees a truncated or absent manifest (issue #1273).
///
/// Field shapes here are load-bearing: `claude plugin validate` rejects a
/// bare-string `author`. Skills are intentionally omitted because direct
/// children of `~/.claude/skills` are the sole discovery path.
fn write_plugin_manifest(plugin_dir: &Path, scratch: &mut Scratch) -> Result<bool> {
    let manifest_dir = plugin_dir.join(".claude-plugin");
    if !manifest_dir.is_dir() {
        fs::create_dir_all(&manifest_dir)
            .with_context(|| format!("failed to create {}", manifest_dir.display()))?;
    }
    let manifest_path = manifest_dir.join("plugin.json");
    let desired = plugin_manifest_bytes()?;
    if file_has_contents(&manifest_path, &desired)? {
        return Ok(false);
    }
    let staged_manifest = scratch.path("plugin.json")?;
    fs::write(&staged_manifest, &desired)
        .with_context(|| format!("failed to write {}", staged_manifest.display()))?;
    if let Err(error) = fs::rename(&staged_manifest, &manifest_path) {
        let _ = fs::remove_file(&staged_manifest);
        return Err(error)
            .with_context(|| format!("failed to publish {}", manifest_path.display()));
    }
    Ok(true)
}

fn plugin_manifest_bytes() -> Result<Vec<u8>> {
    let manifest = json!({
        "$schema": "https://anthropic.com/claude-code/plugin.schema.json",
        "name": PLUGIN_NAME,
        "version": PLUGIN_VERSION,
        "description": "Amplihack AI development framework — agents, skills, and commands.",
        "author": {
            "name": "Microsoft",
        },
    });
    Ok((serde_json::to_string_pretty(&manifest)? + "\n").into_bytes())
}

/// Whether `path` is a regular file that already holds exactly `desired`.
///
/// A symlink or directory answers `false` so the caller falls through to its
/// normal publishing path, which fails loudly rather than following the link.
fn file_has_contents(path: &Path, desired: &[u8]) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(fs::read(path).with_context(|| format!("failed to read {}", path.display()))? == desired)
}

/// A directory that exists and is not a symlink.
fn is_real_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

/// Lazily created scratch space beside the destination being published.
///
/// Nothing is created until a replacement is actually needed, so an up-to-date
/// launch touches the filesystem only to read. It deliberately lives *outside*
/// the plugin wrapper: a crashed launch can then never strand a half-built
/// entry inside the wrapper and invalidate its ownership digest.
struct Scratch {
    root: PathBuf,
    directory: Option<tempfile::TempDir>,
}

impl Scratch {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            directory: None,
        }
    }

    /// A fresh, unused path in the scratch directory, creating it on demand.
    fn path(&mut self, name: &str) -> Result<PathBuf> {
        let directory = match &self.directory {
            Some(directory) => directory,
            None => self.directory.insert(
                tempfile::Builder::new()
                    .prefix(".amplihack-plugin-staging-")
                    .tempdir_in(&self.root)
                    .context("failed to create sibling plugin staging directory")?,
            ),
        };
        Ok(directory
            .path()
            .join(format!("{name}.{}", uuid::Uuid::new_v4())))
    }
}

/// Mirror each asset directory from the staged framework into the plugin
/// dir. Symlink when possible, fall back to a recursive copy otherwise.
///
/// The agents layout needs a special note: Claude Code expects flat files
/// under `agents/` (one markdown per agent). The staged framework puts
/// them under `agents/amplihack/<category>/...`. We mirror the `amplihack`
/// subdirectory directly so `~/.claude/skills/amplihack/agents/<...>`
/// matches Claude Code's discovery pattern.
///
/// Idempotent and non-destructive, and it has to be: this runs on every
/// launch. A mirror that already points where it should is left completely
/// alone, and one that does need replacing is built beside the target and
/// `rename`d over it. Nothing is ever unlinked before its replacement exists,
/// so neither a crash nor a second launch racing this one from another
/// directory can observe a missing asset tree (issue #1273).
///
/// Returns whether any mirror was created, replaced, or removed.
fn mirror_assets(staged: &Path, plugin_dir: &Path, scratch: &mut Scratch) -> Result<bool> {
    let mut changed = false;
    for asset in MIRRORED_ASSETS {
        let source = resolve_asset_source(staged, asset);
        let target = plugin_dir.join(asset);
        if !source.is_dir() {
            // The staged framework no longer ships this asset, so a mirror
            // left by an older amplihack should stop being live.
            changed |= remove_mirror(&target)?;
            continue;
        }
        if mirror_is_current(&source, &target)? {
            continue;
        }
        replace_mirror(&source, &target, scratch)?;
        changed = true;
    }
    Ok(changed)
}

/// Whether `target` already mirrors `source` exactly.
fn mirror_is_current(source: &Path, target: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", target.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        return Ok(fs::read_link(target)
            .with_context(|| format!("failed to read symlink {}", target.display()))?
            == source);
    }
    if !metadata.is_dir() {
        return Ok(false);
    }
    // Copy fallback (a platform where symlinking failed): the mirror is
    // current when it still holds exactly what a fresh copy would produce.
    // Comparing digests costs a read of both trees, which is cheaper than the
    // rewrite it avoids — and, unlike the rewrite, it destroys nothing.
    Ok(hash_directory_tree(target)? == hash_copy_plan(source)?)
}

/// Put a fresh mirror of `source` at `target` without ever leaving that path
/// empty: build the replacement under a scratch name, then rename it over.
fn replace_mirror(source: &Path, target: &Path, scratch: &mut Scratch) -> Result<()> {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("mirror target has an invalid name: {}", target.display()))?;
    let replacement = scratch.path(name)?;
    let built = match try_symlink(source, &replacement) {
        Ok(()) => Ok(()),
        Err(_) => copy_dir_recursive(source, &replacement),
    };
    let result = built.and_then(|()| rename_over(&replacement, target, scratch));
    if result.is_err() {
        let _ = fs::remove_file(&replacement);
        let _ = fs::remove_dir_all(&replacement);
    }
    result
}

/// Move `replacement` onto `target`.
///
/// `rename(2)` swaps a symlink over a symlink or a file in one atomic step,
/// which is every steady-state case: the mirror at `target` is a symlink, and
/// so is its replacement. POSIX refuses the two mixed cases — a symlink over a
/// directory, or a copy-fallback directory over a symlink — so there the old
/// entry is moved aside first. That still happens only *after* the replacement
/// is fully built, so the gap is two adjacent renames rather than the length of
/// a re-copy, and a failed publish puts the previous tree straight back.
fn rename_over(replacement: &Path, target: &Path, scratch: &mut Scratch) -> Result<()> {
    let publish_error = match fs::rename(replacement, target) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    // Nothing is in the way, so the rename failed for its own reasons.
    if fs::symlink_metadata(target).is_err() {
        return Err(publish_error)
            .with_context(|| format!("failed to publish mirror {}", target.display()));
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("mirror target has an invalid name: {}", target.display()))?;
    let displaced = scratch.path(&format!("{name}.replaced"))?;
    fs::rename(target, &displaced)
        .with_context(|| format!("failed to move aside {}", target.display()))?;
    if let Err(error) = fs::rename(replacement, target) {
        let _ = fs::rename(&displaced, target);
        return Err(error)
            .with_context(|| format!("failed to publish mirror {}", target.display()));
    }
    let _ = remove_mirror(&displaced);
    Ok(())
}

/// Remove a mirror amplihack no longer publishes, reporting whether anything
/// was there. Only ever called for paths the ownership digest has vouched for.
fn remove_mirror(target: &Path) -> Result<bool> {
    match fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", target.display()));
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(target)
            .with_context(|| format!("failed to remove stale mirror {}", target.display()))?,
        Ok(_) => fs::remove_file(target)
            .with_context(|| format!("failed to remove stale mirror {}", target.display()))?,
    }
    Ok(true)
}

/// Digest of the tree `copy_dir_recursive` would produce from `source`.
///
/// Deliberately mirrors `hash_directory_tree` over a finished copy: the same
/// directories and regular files in the same order, with symlinks skipped
/// exactly as the copy skips them. That lets a copy-fallback mirror be checked
/// for staleness without deleting it to find out.
fn hash_copy_plan(source: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_copy_plan_entries(source, source, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    hash_tree_entries(source, entries)
}

fn collect_copy_plan_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, TreeEntry)>,
) -> Result<()> {
    for entry in
        fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .context("staged asset entry escaped its root")?
            .to_path_buf();
        if file_type.is_dir() {
            entries.push((relative, TreeEntry::Directory));
            collect_copy_plan_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            entries.push((relative, TreeEntry::File));
        }
    }
    Ok(())
}

/// Resolve the staged source directory for a given asset type, accounting
/// for the amplihack-specific subdirectory layout inside agents/commands.
fn resolve_asset_source(staged: &Path, asset: &str) -> PathBuf {
    // Staged framework nests its own content under `<asset>/amplihack/`.
    // Claude Code plugin dirs want the content directly under `<asset>/`,
    // so we prefer the amplihack-scoped subdir when it exists.
    let scoped = staged.join(asset).join("amplihack");
    if scoped.is_dir() {
        return scoped;
    }
    staged.join(asset)
}

/// Install every canonical skill as a direct child of Claude's discovery
/// directory while leaving user-owned entries untouched.
fn sync_canonical_skills(
    source_root: &Path,
    destination_root: &Path,
    ownership_manifest_path: &Path,
) -> Result<()> {
    let mut skills = Vec::new();
    find_skill_dirs(source_root, &mut skills)?;
    skills.sort_by(|left, right| left.0.cmp(&right.0));

    for pair in skills.windows(2) {
        if pair[0].0 == pair[1].0 {
            bail!(
                "canonical skill name conflict for '{}': {} and {}",
                pair[0].0,
                pair[0].1.display(),
                pair[1].1.display()
            );
        }
    }

    fs::create_dir_all(destination_root)
        .with_context(|| format!("failed to create {}", destination_root.display()))?;

    let owned = read_skill_ownership_manifest(ownership_manifest_path)?;
    let canonical_names = skills
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();

    for (name, _) in &skills {
        validate_skill_destination(
            name,
            &destination_root.join(name),
            owned.skills.iter().find(|entry| entry.name == *name),
        )?;
    }
    for stale in owned
        .skills
        .iter()
        .filter(|entry| !canonical_names.contains(&entry.name))
    {
        validate_skill_destination(
            &stale.name,
            &destination_root.join(&stale.name),
            Some(stale),
        )?;
    }

    let canonical_root = fs::canonicalize(source_root)
        .with_context(|| format!("failed to resolve {}", source_root.display()))?;
    let transaction_dir = tempfile::Builder::new()
        .prefix(".amplihack-skill-transaction-")
        .tempdir_in(destination_root)
        .context("failed to create sibling skill transaction directory")?;
    let staging_root = transaction_dir.path().join("staged");
    let backup_root = transaction_dir.path().join("backups");
    fs::create_dir_all(&staging_root)
        .with_context(|| format!("failed to create {}", staging_root.display()))?;
    fs::create_dir_all(&backup_root)
        .with_context(|| format!("failed to create {}", backup_root.display()))?;

    let mut staged = Vec::with_capacity(skills.len());
    for (name, source) in &skills {
        let staging_dir = staging_root.join(name);
        copy_skill_tree(source, &staging_dir, source_root)
            .with_context(|| format!("failed to stage canonical skill '{name}'"))?;
        staged.push((name.clone(), staging_dir));
    }
    materialize_known_skill_links(source_root, &staging_root, &canonical_root)?;

    // A skill whose freshly staged tree is identical to the installed one
    // needs no republication at all, and republishing it anyway is not free:
    // every swap below moves the live directory aside before moving the new
    // one in, so an unconditional refresh opens one window per skill, on every
    // launch, in which that skill is simply absent (issue #1273).
    //
    // `validate_skill_destination` has already proved that an owned
    // destination still hashes to its recorded digest, so comparing against
    // the record is the same comparison as against the destination — without a
    // second walk of it.
    let mut digests = BTreeMap::new();
    let mut unchanged = BTreeSet::new();
    for (name, staging_dir) in &staged {
        let digest = hash_directory_tree(staging_dir)?;
        let matches_record = owned
            .skills
            .iter()
            .any(|entry| entry.name == *name && entry.content_sha256 == digest);
        if matches_record && is_real_directory(&destination_root.join(name))? {
            unchanged.insert(name.clone());
        }
        digests.insert(name.clone(), digest);
    }

    let mut applied = Vec::new();
    let transaction_result = (|| -> Result<()> {
        for (name, staging_dir) in &staged {
            if unchanged.contains(name) {
                continue;
            }
            let destination = destination_root.join(name);
            let backup = move_destination_to_backup(&destination, &backup_root, name)?;
            applied.push(AppliedSkill {
                destination: destination.clone(),
                backup,
            });
            if let Err(error) = fs::rename(staging_dir, &destination) {
                return Err(error)
                    .with_context(|| format!("failed to publish canonical skill '{name}'"));
            }
        }

        for stale in owned
            .skills
            .iter()
            .filter(|entry| !canonical_names.contains(&entry.name))
        {
            let destination = destination_root.join(&stale.name);
            if destination.symlink_metadata().is_ok() {
                let backup = move_destination_to_backup(&destination, &backup_root, &stale.name)?
                    .context("owned stale skill disappeared during refresh")?;
                applied.push(AppliedSkill {
                    destination,
                    backup: Some(backup),
                });
            }
        }

        // Each published destination is now exactly its staged tree, and each
        // skipped one already was, so the staged digests describe both without
        // re-walking the destinations.
        let skills = canonical_names
            .iter()
            .map(|name| {
                Ok(OwnedDestination {
                    name: name.clone(),
                    content_sha256: digests
                        .get(name)
                        .with_context(|| format!("missing staged digest for skill '{name}'"))?
                        .clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        write_skill_ownership_manifest(
            ownership_manifest_path,
            &SkillOwnershipManifest {
                version: SKILL_OWNERSHIP_VERSION,
                skills,
                plugin: owned.plugin.clone(),
                commands: owned.commands.clone(),
            },
        )
    })();

    if let Err(error) = transaction_result {
        return Err(skill_transaction_error(error, &applied, transaction_dir));
    }

    transaction_dir
        .close()
        .context("failed to remove completed skill transaction directory")
}

fn skill_transaction_error(
    publish_error: anyhow::Error,
    applied: &[AppliedSkill],
    transaction_dir: tempfile::TempDir,
) -> anyhow::Error {
    match rollback_applied_skills(applied) {
        Ok(()) => publish_error,
        Err(rollback_error) => {
            let recovery = transaction_dir.keep();
            publish_error.context(format!(
                "rollback was incomplete: {rollback_error:#}; recover backups from {}",
                recovery.display()
            ))
        }
    }
}

struct AppliedSkill {
    destination: PathBuf,
    backup: Option<PathBuf>,
}

fn move_destination_to_backup(
    destination: &Path,
    backup_root: &Path,
    name: &str,
) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", destination.display()));
        }
    }
    let backup = backup_root.join(format!("{name}-{}", uuid::Uuid::new_v4()));
    fs::rename(destination, &backup).with_context(|| {
        format!(
            "failed to back up existing skill '{}' from {}",
            name,
            destination.display()
        )
    })?;
    Ok(Some(backup))
}

fn rollback_applied_skills(applied: &[AppliedSkill]) -> Result<()> {
    let mut failures = Vec::new();
    for change in applied.iter().rev() {
        let destination_ready = match fs::symlink_metadata(&change.destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                match fs::remove_dir_all(&change.destination) {
                    Ok(()) => true,
                    Err(error) => {
                        failures.push(format!(
                            "failed to remove partial skill {} during rollback: {error}",
                            change.destination.display()
                        ));
                        false
                    }
                }
            }
            Ok(_) => {
                failures.push(format!(
                    "refusing to remove unexpected partial skill type during rollback: {}",
                    change.destination.display()
                ));
                false
            }
            Err(error) if error.kind() == ErrorKind::NotFound => true,
            Err(error) => {
                failures.push(format!(
                    "failed to inspect partial skill {} during rollback: {error}",
                    change.destination.display()
                ));
                false
            }
        };
        if destination_ready
            && let Some(backup) = &change.backup
            && let Err(error) = fs::rename(backup, &change.destination)
        {
            failures.push(format!(
                "failed to restore previous skill {} from {} during rollback: {error}",
                change.destination.display(),
                backup.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("; "))
    }
}

fn read_skill_ownership_manifest(path: &Path) -> Result<SkillOwnershipManifest> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(SkillOwnershipManifest {
                version: SKILL_OWNERSHIP_VERSION,
                skills: Vec::new(),
                plugin: None,
                commands: None,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read skill ownership manifest {}", path.display())
            });
        }
    };
    let manifest: SkillOwnershipManifest = serde_json::from_str(&raw)
        .with_context(|| format!("corrupt skill ownership manifest {}", path.display()))?;
    if manifest.version != SKILL_OWNERSHIP_VERSION {
        bail!(
            "unsupported skill ownership manifest version {} in {}",
            manifest.version,
            path.display()
        );
    }
    let mut owned = BTreeSet::new();
    for skill in &manifest.skills {
        validate_skill_name(&skill.name)?;
        validate_sha256(&skill.content_sha256)?;
        if !owned.insert(skill.name.clone()) {
            bail!(
                "duplicate skill '{}' in ownership manifest {}",
                skill.name,
                path.display()
            );
        }
    }
    if let Some(plugin) = &manifest.plugin {
        if plugin.name != PLUGIN_NAME {
            bail!("invalid Claude plugin identity '{}'", plugin.name);
        }
        validate_sha256(&plugin.content_sha256)?;
    }
    if let Some(commands) = &manifest.commands {
        if commands.name != PLUGIN_NAME {
            bail!(
                "invalid Claude command namespace identity '{}'",
                commands.name
            );
        }
        validate_sha256(&commands.content_sha256)?;
    }
    Ok(manifest)
}

/// Publish the ownership record, skipping the write entirely when the record
/// on disk already says exactly this (issue #1273).
fn write_skill_ownership_manifest(path: &Path, manifest: &SkillOwnershipManifest) -> Result<()> {
    let serialized = serde_json::to_string_pretty(&manifest)? + "\n";
    if file_has_contents(path, serialized.as_bytes())? {
        return Ok(());
    }
    let parent = path
        .parent()
        .context("skill ownership manifest has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{SKILL_OWNERSHIP_MANIFEST}.{}",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, &serialized)
        .with_context(|| format!("failed to write {}", temporary.display()))?;

    let backup = parent.join(format!(
        ".{SKILL_OWNERSHIP_MANIFEST}.backup.{}",
        uuid::Uuid::new_v4()
    ));
    let had_manifest = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::rename(path, &backup).with_context(|| {
                format!("failed to back up ownership manifest {}", path.display())
            })?;
            true
        }
        Ok(_) => {
            let _ = fs::remove_file(&temporary);
            bail!(
                "skill ownership manifest has unsupported type: {}",
                path.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| {
                format!("failed to inspect ownership manifest {}", path.display())
            });
        }
    };
    if let Err(error) = fs::rename(&temporary, path) {
        let restore_error = had_manifest
            .then(|| fs::rename(&backup, path).err())
            .flatten();
        let _ = fs::remove_file(&temporary);
        if let Some(restore_error) = restore_error {
            return Err(error).context(format!(
                "failed to publish ownership manifest {}; immediate restoration also failed: \
                 {restore_error}; recover previous manifest from {}",
                path.display(),
                backup.display()
            ));
        }
        return Err(error)
            .with_context(|| format!("failed to publish ownership manifest {}", path.display()));
    }
    if had_manifest && let Err(error) = fs::remove_file(&backup) {
        tracing::warn!(
            path = %backup.display(),
            %error,
            "published skill ownership manifest but could not remove its obsolete backup"
        );
    }
    Ok(())
}

fn validate_skill_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name == PLUGIN_NAME
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        bail!("invalid canonical skill name '{name}'");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid ownership content digest");
    }
    Ok(())
}

fn hash_directory_tree(root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    hash_tree_entries(root, entries)
}

fn hash_tree_entries(root: &Path, entries: Vec<(PathBuf, TreeEntry)>) -> Result<String> {
    let mut hasher = Sha256::new();
    for (relative, kind) in entries {
        let relative = relative
            .to_str()
            .with_context(|| format!("non-UTF-8 path under {}", root.display()))?;
        hasher.update(match &kind {
            TreeEntry::Directory => b"d",
            TreeEntry::File => b"f",
            TreeEntry::Symlink(_) => b"l",
        });
        hasher.update((relative.len() as u64).to_le_bytes());
        hasher.update(relative.as_bytes());
        match kind {
            TreeEntry::Directory => {}
            TreeEntry::File => {
                let bytes = fs::read(root.join(relative))
                    .with_context(|| format!("failed to hash {}", root.join(relative).display()))?;
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
            TreeEntry::Symlink(link) => {
                let link = link
                    .to_str()
                    .with_context(|| format!("non-UTF-8 symlink under {}", root.display()))?;
                hasher.update((link.len() as u64).to_le_bytes());
                hasher.update(link.as_bytes());
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

enum TreeEntry {
    Directory,
    File,
    Symlink(PathBuf),
}

fn collect_tree_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, TreeEntry)>,
) -> Result<()> {
    for entry in fs::read_dir(current)
        .with_context(|| format!("failed to read managed destination {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let kind = if file_type.is_symlink() {
            TreeEntry::Symlink(
                fs::read_link(&path)
                    .with_context(|| format!("failed to read symlink {}", path.display()))?,
            )
        } else if file_type.is_dir() {
            TreeEntry::Directory
        } else if file_type.is_file() {
            TreeEntry::File
        } else {
            bail!(
                "unsupported entry in managed destination: {}",
                path.display()
            );
        };
        let relative = path
            .strip_prefix(root)
            .context("managed destination entry escaped its root")?
            .to_path_buf();
        let recurse = matches!(kind, TreeEntry::Directory);
        entries.push((relative, kind));
        if recurse {
            collect_tree_entries(root, &path, entries)?;
        }
    }

    Ok(())
}

fn copy_skill_tree(src: &Path, dst: &Path, skills_root: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source = entry.path();
        let target = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_skill_tree(&source, &target, skills_root)?;
        } else if file_type.is_file() {
            if is_known_skill_link_placeholder(&source, skills_root)? {
                continue;
            }
            copy_file(&source, &target)?;
        } else if file_type.is_symlink() {
            let relative = source
                .strip_prefix(skills_root)
                .context("canonical skill symlink escaped its skills root")?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            let Some(link) = KNOWN_SKILL_LINKS.iter().find(|link| link.path == relative) else {
                bail!(
                    "unexpected canonical skill symlink {}; only explicitly bundled support links are allowed",
                    source.display()
                );
            };
            let actual_target = fs::read_link(&source)
                .with_context(|| format!("failed to read symlink {}", source.display()))?;
            if actual_target != Path::new(link.link_target) {
                bail!(
                    "canonical skill symlink {} has unexpected target {}; expected {}",
                    source.display(),
                    actual_target.display(),
                    link.link_target
                );
            }
        }
    }
    Ok(())
}

fn is_known_skill_link_placeholder(source: &Path, skills_root: &Path) -> Result<bool> {
    let relative = match source.strip_prefix(skills_root) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => return Ok(false),
    };
    let Some(link) = KNOWN_SKILL_LINKS.iter().find(|link| link.path == relative) else {
        return Ok(false);
    };
    Ok(
        fs::read(source).with_context(|| format!("failed to read {}", source.display()))?
            == link.link_target.as_bytes(),
    )
}

fn materialize_known_skill_links(
    source_root: &Path,
    destination_root: &Path,
    canonical_root: &Path,
) -> Result<()> {
    for link in KNOWN_SKILL_LINKS {
        let source = source_root.join(link.path);
        match fs::symlink_metadata(&source) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let actual_target = fs::read_link(&source)
                    .with_context(|| format!("failed to read symlink {}", source.display()))?;
                if actual_target != Path::new(link.link_target) {
                    bail!(
                        "canonical skill symlink {} has unexpected target {}; expected {}",
                        source.display(),
                        actual_target.display(),
                        link.link_target
                    );
                }
            }
            Ok(metadata) if metadata.is_file() => {
                if fs::read(&source)
                    .with_context(|| format!("failed to read {}", source.display()))?
                    != link.link_target.as_bytes()
                {
                    bail!(
                        "known skill link {} has an unexpected regular-file representation",
                        source.display()
                    );
                }
            }
            Ok(_) => bail!(
                "known skill link {} has an unsupported source type",
                source.display()
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", source.display()));
            }
        }

        let unresolved_target = source_root.join(link.canonical_target);
        let resolved_target = match fs::canonicalize(&unresolved_target) {
            Ok(target) => target,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to resolve known skill support target {}",
                        unresolved_target.display()
                    )
                });
            }
        };
        if !resolved_target.starts_with(canonical_root) {
            bail!(
                "known skill support target {} points outside {}",
                unresolved_target.display(),
                canonical_root.display()
            );
        }

        let destination = destination_root.join(link.path);
        if destination.symlink_metadata().is_ok() {
            bail!(
                "known skill support destination already exists: {}",
                destination.display()
            );
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        copy_resolved_skill_link(
            &source,
            &resolved_target,
            &destination,
            source_root,
            &mut BTreeSet::new(),
        )?;
    }
    Ok(())
}

fn copy_resolved_skill_link(
    source: &Path,
    resolved: &Path,
    target: &Path,
    skills_root: &Path,
    expanded_links: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if !expanded_links.insert(resolved.to_path_buf()) {
        bail!(
            "canonical skill link {} repeats target {}",
            source.display(),
            resolved.display()
        );
    }
    let resolved_metadata = fs::metadata(resolved)?;
    if resolved_metadata.is_dir() {
        copy_skill_tree(resolved, target, skills_root)
    } else if resolved_metadata.is_file() {
        copy_file(resolved, target)
    } else {
        bail!(
            "canonical skill link {} resolves to unsupported file type",
            source.display()
        )
    }
}

fn copy_file(source: &Path, target: &Path) -> Result<()> {
    fs::copy(source, target).map(|_| ()).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            target.display()
        )
    })
}

fn find_skill_dirs(root: &Path, skills: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }

    let mut contains_skill_manifest = false;
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_file() && entry.file_name() == "SKILL.md" {
            contains_skill_manifest = true;
        } else if file_type.is_dir() {
            find_skill_dirs(&entry.path(), skills)?;
        }
    }

    if contains_skill_manifest {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("canonical skill has an invalid name: {}", root.display()))?;
        validate_skill_name(name)?;
        skills.push((name.to_owned(), root.to_path_buf()));
    }
    Ok(())
}

fn validate_skill_destination(
    name: &str,
    destination: &Path,
    owned: Option<&OwnedDestination>,
) -> Result<()> {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect skill destination '{}'", name));
        }
    };

    if metadata.file_type().is_symlink() {
        bail!(
            "skill '{}' conflicts with destination symlink {}; remove it before installing",
            name,
            destination.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "skill '{}' conflicts with user-owned destination {}; move it before installing",
            name,
            destination.display()
        );
    }

    let Some(owned) = owned else {
        bail!(
            "skill '{}' conflicts with user-owned directory {}; move it before installing",
            name,
            destination.display()
        );
    };

    let actual = hash_directory_tree(destination)?;
    if actual != owned.content_sha256 {
        bail!(
            "skill '{}' no longer matches amplihack ownership state at {}; preserving replaced content",
            name,
            destination.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn try_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target)
        .with_context(|| format!("symlink {} -> {}", target.display(), source.display()))
}

#[cfg(windows)]
fn try_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
        .with_context(|| format!("symlink {} -> {}", target.display(), source.display()))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if file_type.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        // Symlinks inside the staged framework are skipped deliberately —
        // they point back into the framework source and don't need to be
        // re-exposed through the plugin dir.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn isolated_home() -> (
        std::sync::MutexGuard<'static, ()>,
        tempfile::TempDir,
        crate::test_support::HomeGuard,
    ) {
        let lock = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let home = crate::test_support::HomeGuard::set(temp.path());
        (lock, temp, home)
    }

    fn write_bundled_skill(home: &Path, relative_path: &str, skill_body: &str) -> PathBuf {
        let skill_dir = home.join(".amplihack/.claude/skills").join(relative_path);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), skill_body).unwrap();
        skill_dir
    }

    fn write_legacy_plugin_wrapper(staged: &Path, plugin_dir: &Path) {
        fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".claude-plugin/plugin.json"),
            serde_json::to_string_pretty(&json!({
                "$schema": "https://anthropic.com/claude-code/plugin.schema.json",
                "name": "amplihack",
                "version": "0.4.2",
                "description": "Amplihack AI development framework — agents, skills, and commands.",
                "author": {"name": "Microsoft"},
                "skills": ["./skills"],
            }))
            .unwrap(),
        )
        .unwrap();
        for asset in ["agents", "skills", "commands", "context", "workflow"] {
            let source = resolve_asset_source(staged, asset);
            if source.is_dir() {
                let target = plugin_dir.join(asset);
                if try_symlink(&source, &target).is_err() {
                    copy_dir_recursive(&source, &target).unwrap();
                }
            }
        }
    }

    fn copy_tree_as_windows_checkout(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            let file_type = entry.file_type().unwrap();
            if file_type.is_symlink() {
                let target = fs::read_link(&source_path).unwrap();
                fs::write(destination_path, target.to_string_lossy().as_bytes()).unwrap();
            } else if file_type.is_dir() {
                copy_tree_as_windows_checkout(&source_path, &destination_path);
            } else if file_type.is_file() {
                fs::copy(source_path, destination_path).unwrap();
            }
        }
    }

    #[test]
    fn resolve_asset_source_prefers_scoped_subdir() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path();
        fs::create_dir_all(staged.join("agents/amplihack")).unwrap();
        let src = resolve_asset_source(staged, "agents");
        assert_eq!(src, staged.join("agents/amplihack"));
    }

    #[test]
    fn resolve_asset_source_falls_back_to_flat_dir() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path();
        fs::create_dir_all(staged.join("skills")).unwrap();
        let src = resolve_asset_source(staged, "skills");
        assert_eq!(src, staged.join("skills"));
    }

    #[test]
    fn ensure_claude_plugin_installed_is_noop_without_staging() {
        let (_lock, temp, _home) = isolated_home();
        ensure_claude_plugin_installed().unwrap();
        // No plugin dir should be created when staging is absent.
        assert!(!temp.path().join(".claude/skills/amplihack").exists());
    }

    #[test]
    fn issue_1277_adopts_current_release_legacy_wrapper_layout() {
        let (_lock, temp, _home) = isolated_home();

        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::create_dir_all(staged.join("skills/dev-orchestrator")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent").unwrap();
        fs::write(staged.join("skills/dev-orchestrator/SKILL.md"), "skill").unwrap();
        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        write_legacy_plugin_wrapper(&staged, &plugin_dir);

        ensure_claude_plugin_installed().unwrap();

        let manifest_path = plugin_dir.join(".claude-plugin/plugin.json");
        assert!(manifest_path.is_file());
        assert!(plugin_dir.join("agents/core/architect.md").exists());
        assert!(!plugin_dir.join("skills").exists());
        assert!(
            temp.path()
                .join(".claude/skills/dev-orchestrator/SKILL.md")
                .is_file()
        );

        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert!(manifest.get("author").unwrap().is_object());
        assert!(manifest.get("skills").is_none());
    }

    #[test]
    fn issue_1277_adopts_real_windows_legacy_wrapper_after_link_skipping() {
        let (_lock, temp, _home) = isolated_home();
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let real_skills = workspace.join("amplifier-bundle/skills");
        let staged = temp.path().join(".amplihack/.claude");
        copy_dir_recursive(&real_skills, &staged.join("skills")).unwrap();

        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        write_legacy_plugin_wrapper(&staged, &plugin_dir);
        let legacy_skills = plugin_dir.join("skills");
        if legacy_skills
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
        {
            fs::remove_file(&legacy_skills).unwrap();
        } else {
            fs::remove_dir_all(&legacy_skills).unwrap();
        }
        copy_tree_as_windows_checkout(&real_skills, &legacy_skills);
        assert_eq!(
            fs::read_to_string(legacy_skills.join("docx/ooxml")).unwrap(),
            "../common/ooxml"
        );
        assert!(!staged.join("skills/docx/ooxml").exists());

        ensure_claude_plugin_installed().unwrap();

        assert!(!plugin_dir.join("skills").exists());
        assert!(temp.path().join(".claude/skills/docx/SKILL.md").is_file());
    }

    #[test]
    fn issue_1277_legacy_link_exception_rejects_unrelated_contents() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        for path in ["skills/docx", "skills/common/ooxml"] {
            fs::create_dir_all(staged.join(path)).unwrap();
        }
        fs::write(staged.join("skills/docx/SKILL.md"), "skill").unwrap();
        fs::write(staged.join("skills/common/ooxml/tool.txt"), "canonical").unwrap();
        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        write_legacy_plugin_wrapper(&staged, &plugin_dir);
        let legacy_skills = plugin_dir.join("skills");
        if legacy_skills
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
        {
            fs::remove_file(&legacy_skills).unwrap();
            copy_dir_recursive(&staged.join("skills"), &legacy_skills).unwrap();
        }
        fs::write(legacy_skills.join("docx/ooxml"), "not a link placeholder").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("unowned destination"));
        assert_eq!(
            fs::read_to_string(legacy_skills.join("docx/ooxml")).unwrap(),
            "not a link placeholder"
        );
    }

    #[test]
    fn issue_1277_legacy_wrapper_adoption_fails_closed_for_extra_content() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        write_bundled_skill(temp.path(), "quality-audit", "canonical\n");
        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        write_legacy_plugin_wrapper(&staged, &plugin_dir);
        fs::write(plugin_dir.join("user-notes.txt"), "preserve\n").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("unowned destination"));
        assert_eq!(
            fs::read_to_string(plugin_dir.join("user-notes.txt")).unwrap(),
            "preserve\n"
        );
        assert!(!temp.path().join(".claude/skills/quality-audit").exists());
    }

    #[test]
    fn issue_1277_failed_wrapper_publication_leaves_no_live_wrapper_and_retries() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("skills/quality-audit")).unwrap();
        fs::write(staged.join("skills/quality-audit/SKILL.md"), "canonical\n").unwrap();
        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        let ownership_path = skill_ownership_manifest_path(&staged);
        fs::create_dir_all(&ownership_path).unwrap();

        let error = publish_plugin_wrapper(&staged, &plugin_dir, &ownership_path).unwrap_err();
        assert!(format!("{error:#}").contains("failed to read skill ownership manifest"));
        assert!(
            !plugin_dir.exists(),
            "a failed first publication must not leave an unowned live wrapper"
        );

        fs::remove_dir(&ownership_path).unwrap();
        publish_plugin_wrapper(&staged, &plugin_dir, &ownership_path).unwrap();
        let installed: Value = serde_json::from_str(
            &fs::read_to_string(plugin_dir.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert!(installed.get("skills").is_none());
        assert!(
            read_skill_ownership_manifest(&ownership_path)
                .unwrap()
                .plugin
                .is_some()
        );
    }

    #[test]
    fn issue_1277_stages_top_level_and_nested_skills_with_all_support_files() {
        let (_lock, temp, _home) = isolated_home();
        let top_level = write_bundled_skill(
            temp.path(),
            "dev-orchestrator",
            "---\nname: dev-orchestrator\n---\n",
        );
        fs::create_dir_all(top_level.join("scripts")).unwrap();
        fs::write(top_level.join("reference.md"), "# Reference\n").unwrap();
        fs::write(top_level.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();

        let nested = write_bundled_skill(
            temp.path(),
            "development/agentic-workflow-first",
            "---\nname: agentic-workflow-first\n---\n",
        );
        fs::create_dir_all(nested.join("examples")).unwrap();
        fs::write(nested.join("examples/workflow.yaml"), "steps: []\n").unwrap();

        ensure_claude_plugin_installed().unwrap();

        let claude_skills = temp.path().join(".claude/skills");
        assert_eq!(
            fs::read_to_string(claude_skills.join("dev-orchestrator/SKILL.md")).unwrap(),
            "---\nname: dev-orchestrator\n---\n"
        );
        assert_eq!(
            fs::read_to_string(claude_skills.join("dev-orchestrator/reference.md")).unwrap(),
            "# Reference\n"
        );
        assert_eq!(
            fs::read_to_string(claude_skills.join("dev-orchestrator/scripts/run.sh")).unwrap(),
            "#!/bin/sh\n"
        );
        assert_eq!(
            fs::read_to_string(claude_skills.join("agentic-workflow-first/SKILL.md")).unwrap(),
            "---\nname: agentic-workflow-first\n---\n"
        );
        assert_eq!(
            fs::read_to_string(claude_skills.join("agentic-workflow-first/examples/workflow.yaml"))
                .unwrap(),
            "steps: []\n"
        );
        assert!(
            !claude_skills
                .join("development/agentic-workflow-first")
                .exists(),
            "nested category skills must be staged by skill name at the direct discovery root"
        );
    }

    #[test]
    fn issue_1277_reruns_converge_to_the_canonical_skill_contents() {
        let (_lock, temp, _home) = isolated_home();
        let canonical = write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\nversion one\n",
        );

        ensure_claude_plugin_installed().unwrap();
        fs::write(
            canonical.join("SKILL.md"),
            "---\nname: quality-audit\n---\nversion two\n",
        )
        .unwrap();
        fs::write(canonical.join("checklist.txt"), "complete\n").unwrap();

        ensure_claude_plugin_installed().unwrap();

        let installed = temp.path().join(".claude/skills/quality-audit");
        assert_eq!(
            fs::read_to_string(installed.join("SKILL.md")).unwrap(),
            "---\nname: quality-audit\n---\nversion two\n"
        );
        assert_eq!(
            fs::read_to_string(installed.join("checklist.txt")).unwrap(),
            "complete\n"
        );
    }

    #[test]
    fn issue_1277_preserves_unrelated_user_skills_and_root_content() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "amplihack-expert",
            "---\nname: amplihack-expert\n---\n",
        );
        let user_skill = temp.path().join(".claude/skills/my-private-skill");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), "user-owned skill\n").unwrap();
        fs::write(
            temp.path().join(".claude/skills/user-notes.txt"),
            "preserve me\n",
        )
        .unwrap();

        ensure_claude_plugin_installed().unwrap();

        assert_eq!(
            fs::read_to_string(user_skill.join("SKILL.md")).unwrap(),
            "user-owned skill\n"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(".claude/skills/user-notes.txt")).unwrap(),
            "preserve me\n"
        );
        assert!(
            temp.path()
                .join(".claude/skills/amplihack-expert/SKILL.md")
                .is_file()
        );
    }

    #[test]
    fn issue_1277_rejects_a_conflicting_user_owned_skill_name() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "dev-orchestrator",
            "---\nname: dev-orchestrator\n---\nbundled\n",
        );
        let collision = temp.path().join(".claude/skills/dev-orchestrator");
        fs::create_dir_all(&collision).unwrap();
        fs::write(collision.join("SKILL.md"), "user-owned\n").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        let message = format!("{error:#}");
        assert!(
            message.contains("dev-orchestrator") && message.contains("conflict"),
            "collision error must identify the skill and conflict: {message}"
        );
        assert_eq!(
            fs::read_to_string(collision.join("SKILL.md")).unwrap(),
            "user-owned\n",
            "a conflicting user skill must never be overwritten"
        );
    }

    #[test]
    fn issue_1277_does_not_trust_an_in_directory_marker() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "dev-orchestrator",
            "---\nname: dev-orchestrator\n---\nbundled\n",
        );
        let collision = temp.path().join(".claude/skills/dev-orchestrator");
        fs::create_dir_all(&collision).unwrap();
        fs::write(collision.join("SKILL.md"), "user-owned\n").unwrap();
        fs::write(collision.join(".amplihack-managed"), "amplihack\n").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("user-owned"));
        assert_eq!(
            fs::read_to_string(collision.join("SKILL.md")).unwrap(),
            "user-owned\n"
        );
    }

    #[test]
    fn issue_1277_corrupt_or_unsupported_ownership_manifest_fails_closed() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\n",
        );
        let staged = temp.path().join(".amplihack/.claude");
        let manifest = skill_ownership_manifest_path(&staged);
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&manifest, "{not json").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();
        assert!(format!("{error:#}").contains("corrupt skill ownership manifest"));
        assert!(!temp.path().join(".claude/skills/quality-audit").exists());

        fs::write(&manifest, r#"{"version":3,"skills":[]}"#).unwrap();
        let error = ensure_claude_plugin_installed().unwrap_err();
        assert!(format!("{error:#}").contains("unsupported skill ownership manifest version"));
        assert!(!temp.path().join(".claude/skills/quality-audit").exists());
    }

    #[test]
    fn issue_1277_prunes_only_manifest_owned_stale_skills() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(temp.path(), "kept", "---\nname: kept\n---\n");
        let removed = write_bundled_skill(temp.path(), "removed", "---\nname: removed\n---\n");
        ensure_claude_plugin_installed().unwrap();

        fs::remove_dir_all(removed).unwrap();
        let user_skill = temp.path().join(".claude/skills/user-owned");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), "mine").unwrap();
        ensure_claude_plugin_installed().unwrap();

        assert!(temp.path().join(".claude/skills/kept/SKILL.md").is_file());
        assert!(!temp.path().join(".claude/skills/removed").exists());
        assert_eq!(
            fs::read_to_string(user_skill.join("SKILL.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn issue_1277_replaced_managed_skill_fails_closed() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(temp.path(), "kept", "---\nname: kept\n---\ncanonical\n");
        ensure_claude_plugin_installed().unwrap();
        let installed = temp.path().join(".claude/skills/kept/SKILL.md");
        fs::write(&installed, "user replacement\n").unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("no longer matches"));
        assert_eq!(fs::read_to_string(installed).unwrap(), "user replacement\n");
    }

    #[test]
    fn issue_1277_forged_ownership_digest_cannot_authorize_overwrite() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(temp.path(), "kept", "---\nname: kept\n---\ncanonical\n");
        let installed = temp.path().join(".claude/skills/kept");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("SKILL.md"), "user content\n").unwrap();
        let manifest = skill_ownership_manifest_path(&temp.path().join(".amplihack/.claude"));
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            manifest,
            r#"{"version":2,"skills":[{"name":"kept","content_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]}"#,
        )
        .unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("no longer matches"));
        assert_eq!(
            fs::read_to_string(installed.join("SKILL.md")).unwrap(),
            "user content\n"
        );
    }

    #[test]
    fn rollback_attempts_later_restorations_after_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_destination = temp.path().join("blocked");
        let blocked_backup = temp.path().join("blocked-backup");
        fs::write(&blocked_destination, "unexpected").unwrap();
        fs::create_dir_all(&blocked_backup).unwrap();
        fs::write(blocked_backup.join("SKILL.md"), "old blocked").unwrap();

        let restorable_destination = temp.path().join("restored");
        let restorable_backup = temp.path().join("restored-backup");
        fs::create_dir_all(&restorable_backup).unwrap();
        fs::write(restorable_backup.join("SKILL.md"), "old restored").unwrap();
        let changes = vec![
            AppliedSkill {
                destination: restorable_destination.clone(),
                backup: Some(restorable_backup),
            },
            AppliedSkill {
                destination: blocked_destination,
                backup: Some(blocked_backup.clone()),
            },
        ];

        let error = rollback_applied_skills(&changes).unwrap_err();

        assert!(format!("{error:#}").contains("unexpected partial skill type"));
        assert_eq!(
            fs::read_to_string(restorable_destination.join("SKILL.md")).unwrap(),
            "old restored"
        );
        assert!(
            blocked_backup.exists(),
            "unrestored backup must be retained"
        );
    }

    #[test]
    fn incomplete_rollback_reports_both_failures_and_retains_transaction() {
        let root = tempfile::tempdir().unwrap();
        let transaction = tempfile::tempdir_in(root.path()).unwrap();
        let recovery = transaction.path().to_path_buf();
        let destination = root.path().join("unexpected");
        let backup = transaction.path().join("backups/original");
        fs::write(&destination, "unexpected type").unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("SKILL.md"), "recover me").unwrap();
        let applied = [AppliedSkill {
            destination,
            backup: Some(backup.clone()),
        }];

        let error =
            skill_transaction_error(anyhow::anyhow!("publish failed"), &applied, transaction);
        let message = format!("{error:#}");

        assert!(message.contains("publish failed"));
        assert!(message.contains("rollback was incomplete"));
        assert!(message.contains(&recovery.display().to_string()));
        assert!(backup.join("SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn issue_1277_failed_refresh_preserves_previous_skill_and_is_retryable() {
        let (_lock, temp, _home) = isolated_home();
        let source = write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\nworking\n",
        );
        ensure_claude_plugin_installed().unwrap();
        let installed = temp.path().join(".claude/skills/quality-audit/SKILL.md");

        fs::write(
            source.join("SKILL.md"),
            "---\nname: quality-audit\n---\nreplacement\n",
        )
        .unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, source.join("escape")).unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();
        assert!(format!("{error:#}").contains("unexpected canonical skill symlink"));
        assert!(fs::read_to_string(&installed).unwrap().contains("working"));

        fs::remove_file(source.join("escape")).unwrap();
        ensure_claude_plugin_installed().unwrap();
        assert!(
            fs::read_to_string(&installed)
                .unwrap()
                .contains("replacement")
        );
    }

    #[cfg(unix)]
    #[test]
    fn issue_1277_rejects_unexpected_skill_links() {
        let (_lock, temp, _home) = isolated_home();
        let source = write_bundled_skill(temp.path(), "looping", "---\nname: looping\n---\n");
        std::os::unix::fs::symlink(".", source.join("loop")).unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("unexpected canonical skill symlink"));
        assert!(!temp.path().join(".claude/skills/looping").exists());
    }

    #[cfg(unix)]
    #[test]
    fn issue_1277_known_support_target_must_remain_contained() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(temp.path(), "docx", "---\nname: docx\n---\n");
        let outside = temp.path().join("outside-ooxml");
        fs::create_dir_all(&outside).unwrap();
        let common = temp.path().join(".amplihack/.claude/skills/common");
        fs::create_dir_all(&common).unwrap();
        std::os::unix::fs::symlink(&outside, common.join("ooxml")).unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(format!("{error:#}").contains("points outside"));
        assert!(!temp.path().join(".claude/skills/docx").exists());
    }

    #[cfg(unix)]
    #[test]
    fn issue_1277_rejects_destination_symlinks_without_following_them() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\n",
        );
        let external = temp.path().join("external-user-skill");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("SKILL.md"), "external user content\n").unwrap();
        let destination = temp.path().join(".claude/skills/quality-audit");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&external, &destination).unwrap();

        let error = ensure_claude_plugin_installed().unwrap_err();

        assert!(
            format!("{error:#}").contains("quality-audit"),
            "symlink collision error must identify the affected skill"
        );
        assert_eq!(
            fs::read_to_string(external.join("SKILL.md")).unwrap(),
            "external user content\n"
        );
    }

    /// A stable fingerprint of one filesystem entry.
    ///
    /// Inode plus ctime catches every way the launch path could touch a path:
    /// a rewrite in place moves mtime/ctime, a temp-file-and-rename moves the
    /// inode, and an unlink-and-recreate moves both. A directory's own ctime
    /// moves when any child is added or removed, so unlinks show up too.
    #[cfg(unix)]
    fn observe_entry(path: &Path) -> String {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)
            .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));
        format!(
            "ino={} ctime={}.{} mtime={}.{} size={} nlink={}",
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.size(),
            metadata.nlink(),
        )
    }

    #[cfg(unix)]
    fn observe_tree(root: &Path) -> Vec<String> {
        let mut observed = vec![format!(". {}", observe_entry(root))];
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                observed.push(format!(
                    "{} {}",
                    path.strip_prefix(root).unwrap().display(),
                    observe_entry(&path)
                ));
                if fs::symlink_metadata(&path).unwrap().is_dir() {
                    pending.push(path);
                }
            }
        }
        observed.sort();
        observed
    }

    fn write_staged_assets(staged: &Path) {
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent\n").unwrap();
        fs::create_dir_all(staged.join("context")).unwrap();
        fs::write(staged.join("context/PHILOSOPHY.md"), "philosophy\n").unwrap();
    }

    /// Acceptance criterion 1 of issue #1273: a second consecutive launch with
    /// nothing to change writes nothing and unlinks nothing.
    #[cfg(unix)]
    #[test]
    fn issue_1273_second_launch_performs_no_writes_and_no_unlinks() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        write_staged_assets(&staged);
        write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\n",
        );

        ensure_claude_plugin_installed().unwrap();

        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        let skill_dir = temp.path().join(".claude/skills/quality-audit");
        let ownership_path = skill_ownership_manifest_path(&staged);
        let wrapper_before = observe_tree(&plugin_dir);
        let skill_before = observe_tree(&skill_dir);
        let ownership_before = observe_entry(&ownership_path);

        ensure_claude_plugin_installed().unwrap();

        assert_eq!(
            wrapper_before,
            observe_tree(&plugin_dir),
            "an unchanged launch must not rewrite or re-link anything under the plugin wrapper"
        );
        assert_eq!(
            skill_before,
            observe_tree(&skill_dir),
            "an unchanged launch must not republish an already-current canonical skill"
        );
        assert_eq!(
            ownership_before,
            observe_entry(&ownership_path),
            "an unchanged launch must not rewrite the ownership record"
        );
        assert_eq!(
            fs::read_link(plugin_dir.join("agents")).unwrap(),
            staged.join("agents/amplihack"),
            "leaving the wrapper alone must still leave it correct"
        );
    }

    /// Acceptance criterion 2: the pieces that genuinely need repair still get
    /// repaired on a later launch.
    #[cfg(unix)]
    #[test]
    fn issue_1273_a_moved_or_new_asset_source_is_still_mirrored() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents")).unwrap();
        fs::write(staged.join("agents/architect.md"), "flat agent\n").unwrap();
        write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\n",
        );
        ensure_claude_plugin_installed().unwrap();

        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        assert_eq!(
            fs::read_link(plugin_dir.join("agents")).unwrap(),
            staged.join("agents")
        );
        assert!(!plugin_dir.join("workflow").exists());

        // The staged layout moves under the amplihack-scoped subdir, and a
        // brand new asset type appears.
        fs::create_dir_all(staged.join("agents/amplihack")).unwrap();
        fs::write(staged.join("agents/amplihack/architect.md"), "scoped\n").unwrap();
        fs::create_dir_all(staged.join("workflow")).unwrap();
        fs::write(staged.join("workflow/DEFAULT.md"), "workflow\n").unwrap();

        ensure_claude_plugin_installed().unwrap();

        assert_eq!(
            fs::read_link(plugin_dir.join("agents")).unwrap(),
            staged.join("agents/amplihack"),
            "a mirror pointing at the wrong source must be repointed"
        );
        assert_eq!(
            fs::read_link(plugin_dir.join("workflow")).unwrap(),
            staged.join("workflow"),
            "a newly staged asset must be mirrored"
        );
        assert_eq!(
            read_skill_ownership_manifest(&skill_ownership_manifest_path(&staged))
                .unwrap()
                .plugin
                .unwrap()
                .content_sha256,
            hash_directory_tree(&plugin_dir).unwrap(),
            "a repaired wrapper must leave the ownership record consistent"
        );
    }

    /// Acceptance criterion 3: a stale version in plugin.json is still
    /// rewritten, even though an unchanged one no longer is.
    #[test]
    fn issue_1273_a_stale_manifest_version_is_still_rewritten() {
        let (_lock, temp, _home) = isolated_home();
        let staged = temp.path().join(".amplihack/.claude");
        write_staged_assets(&staged);
        write_bundled_skill(
            temp.path(),
            "quality-audit",
            "---\nname: quality-audit\n---\n",
        );
        ensure_claude_plugin_installed().unwrap();

        // Stand in for "installed by an older amplihack": roll the recorded
        // version back and re-record ownership so the wrapper still validates.
        let plugin_dir = temp.path().join(".claude/skills/amplihack");
        let manifest_path = plugin_dir.join(".claude-plugin/plugin.json");
        let mut manifest: Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["version"], json!(PLUGIN_VERSION));
        manifest["version"] = json!("0.0.1-previous");
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
        let ownership_path = skill_ownership_manifest_path(&staged);
        let mut ownership = read_skill_ownership_manifest(&ownership_path).unwrap();
        ownership.plugin = Some(OwnedDestination {
            name: PLUGIN_NAME.to_owned(),
            content_sha256: hash_directory_tree(&plugin_dir).unwrap(),
        });
        write_skill_ownership_manifest(&ownership_path, &ownership).unwrap();

        ensure_claude_plugin_installed().unwrap();

        let installed: Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(installed["version"], json!(PLUGIN_VERSION));
        assert_eq!(
            read_skill_ownership_manifest(&ownership_path)
                .unwrap()
                .plugin
                .unwrap()
                .content_sha256,
            hash_directory_tree(&plugin_dir).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn issue_1273_plugin_manifest_is_written_only_when_it_would_change() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("wrapper");
        let manifest_path = plugin_dir.join(".claude-plugin/plugin.json");
        let mut scratch = Scratch::new(temp.path());

        assert!(write_plugin_manifest(&plugin_dir, &mut scratch).unwrap());
        let published = observe_entry(&manifest_path);

        assert!(
            !write_plugin_manifest(&plugin_dir, &mut scratch).unwrap(),
            "an already-current manifest must not be rewritten"
        );
        assert_eq!(published, observe_entry(&manifest_path));

        fs::write(&manifest_path, "{}\n").unwrap();
        assert!(
            write_plugin_manifest(&plugin_dir, &mut scratch).unwrap(),
            "a manifest whose bytes differ must still be republished"
        );
        assert_eq!(
            fs::read(&manifest_path).unwrap(),
            plugin_manifest_bytes().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn issue_1273_mirror_assets_creates_missing_and_repoints_wrong_links() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        for asset in MIRRORED_ASSETS {
            fs::create_dir_all(staged.join(asset)).unwrap();
        }
        let plugin_dir = temp.path().join("wrapper");
        fs::create_dir_all(&plugin_dir).unwrap();
        let mut scratch = Scratch::new(temp.path());

        assert!(mirror_assets(&staged, &plugin_dir, &mut scratch).unwrap());
        for asset in MIRRORED_ASSETS {
            assert_eq!(
                fs::read_link(plugin_dir.join(asset)).unwrap(),
                staged.join(asset)
            );
        }
        let published = observe_tree(&plugin_dir);

        assert!(
            !mirror_assets(&staged, &plugin_dir, &mut scratch).unwrap(),
            "correct mirrors must be left completely alone"
        );
        assert_eq!(published, observe_tree(&plugin_dir));

        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::remove_file(plugin_dir.join("agents")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, plugin_dir.join("agents")).unwrap();
        fs::remove_file(plugin_dir.join("context")).unwrap();

        assert!(mirror_assets(&staged, &plugin_dir, &mut scratch).unwrap());
        assert_eq!(
            fs::read_link(plugin_dir.join("agents")).unwrap(),
            staged.join("agents"),
            "a mirror pointing somewhere else must be repointed"
        );
        assert_eq!(
            fs::read_link(plugin_dir.join("context")).unwrap(),
            staged.join("context"),
            "a missing mirror must be recreated"
        );

        fs::remove_dir(staged.join("workflow")).unwrap();
        assert!(mirror_assets(&staged, &plugin_dir, &mut scratch).unwrap());
        assert!(
            !plugin_dir.join("workflow").symlink_metadata().is_ok(),
            "a mirror whose source is gone must not stay live"
        );
    }

    #[test]
    fn issue_1273_copy_fallback_mirror_is_reused_when_current() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/guide.md"), "content\n").unwrap();
        let target = temp.path().join("target");
        copy_dir_recursive(&source, &target).unwrap();

        assert!(
            mirror_is_current(&source, &target).unwrap(),
            "a copy-fallback mirror that still matches must not be rebuilt"
        );

        fs::write(source.join("nested/guide.md"), "changed\n").unwrap();
        assert!(
            !mirror_is_current(&source, &target).unwrap(),
            "a stale copy-fallback mirror must be detected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn issue_1273_replaces_a_populated_copy_mirror_with_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("guide.md"), "current\n").unwrap();
        let plugin_dir = temp.path().join("wrapper");
        let target = plugin_dir.join("context");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale.md"), "stale\n").unwrap();
        let mut scratch = Scratch::new(temp.path());

        replace_mirror(&source, &target, &mut scratch).unwrap();

        assert_eq!(fs::read_link(&target).unwrap(), source);
        assert!(target.join("guide.md").is_file());
        assert!(!target.join("stale.md").exists());
    }

    /// The mirror image of the case above: a platform that has stopped being
    /// able to symlink falls back to a copy, and `rename(2)` will not put a
    /// directory over the symlink that is already there. The old entry has to
    /// be moved aside — and it must still never be removed before the
    /// replacement exists.
    #[cfg(unix)]
    #[test]
    fn issue_1273_replaces_a_link_mirror_with_a_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("guide.md"), "current\n").unwrap();
        let plugin_dir = temp.path().join("wrapper");
        fs::create_dir_all(&plugin_dir).unwrap();
        let target = plugin_dir.join("context");
        std::os::unix::fs::symlink(temp.path().join("elsewhere"), &target).unwrap();
        let mut scratch = Scratch::new(temp.path());

        // Stand in for `try_symlink` failing: hand `rename_over` a directory.
        let replacement = scratch.path("context").unwrap();
        copy_dir_recursive(&source, &replacement).unwrap();
        rename_over(&replacement, &target, &mut scratch).unwrap();

        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        assert_eq!(fs::read(target.join("guide.md")).unwrap(), b"current\n");
    }

    /// Acceptance criterion: no point in a launch leaves a mirrored asset
    /// absent, including when two launches race from different directories.
    #[cfg(unix)]
    #[test]
    fn issue_1273_concurrent_mirroring_never_exposes_a_missing_asset_tree() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("wrapper");
        fs::create_dir_all(&plugin_dir).unwrap();
        let sources = [temp.path().join("first"), temp.path().join("second")];
        for source in &sources {
            for asset in MIRRORED_ASSETS {
                fs::create_dir_all(source.join(asset)).unwrap();
            }
        }
        let mut scratch = Scratch::new(temp.path());
        mirror_assets(&sources[0], &plugin_dir, &mut scratch).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let watching = Arc::clone(&stop);
        let watched = plugin_dir.clone();
        let observer = std::thread::spawn(move || {
            while !watching.load(Ordering::Relaxed) {
                for asset in MIRRORED_ASSETS {
                    assert!(
                        watched.join(asset).symlink_metadata().is_ok(),
                        "asset tree '{asset}' vanished while another launch was mirroring"
                    );
                }
            }
        });

        let writers = (0..2usize)
            .map(|writer| {
                let plugin_dir = plugin_dir.clone();
                let sources = sources.clone();
                let root = temp.path().to_path_buf();
                std::thread::spawn(move || {
                    let mut scratch = Scratch::new(&root);
                    for round in 0..200usize {
                        let source = &sources[(writer + round) % sources.len()];
                        mirror_assets(source, &plugin_dir, &mut scratch).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        observer.join().unwrap();

        for asset in MIRRORED_ASSETS {
            let target = fs::read_link(plugin_dir.join(asset)).unwrap();
            assert!(
                sources.iter().any(|source| source.join(asset) == target),
                "racing launches converged on an unexpected target {}",
                target.display()
            );
        }
    }

    #[test]
    fn issue_1277_claude_staging_never_changes_copilot_skills() {
        let (_lock, temp, _home) = isolated_home();
        write_bundled_skill(
            temp.path(),
            "dev-orchestrator",
            "---\nname: dev-orchestrator\n---\nclaude source\n",
        );
        let copilot_skill = temp.path().join(".copilot/skills/dev-orchestrator");
        fs::create_dir_all(&copilot_skill).unwrap();
        fs::write(copilot_skill.join("SKILL.md"), "copilot sentinel\n").unwrap();
        fs::write(copilot_skill.join("copilot-only.txt"), "unchanged\n").unwrap();

        ensure_claude_plugin_installed().unwrap();

        assert_eq!(
            fs::read_to_string(copilot_skill.join("SKILL.md")).unwrap(),
            "copilot sentinel\n"
        );
        assert_eq!(
            fs::read_to_string(copilot_skill.join("copilot-only.txt")).unwrap(),
            "unchanged\n"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(".claude/skills/dev-orchestrator/SKILL.md"))
                .unwrap(),
            "---\nname: dev-orchestrator\n---\nclaude source\n"
        );
    }
}
