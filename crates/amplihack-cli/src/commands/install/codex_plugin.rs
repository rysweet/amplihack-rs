//! Native Codex package and exact-owned user hook registration.
//! Canonical skills remain provider-neutral; generated instruction skills are
//! installation outputs. Native trust is deliberately left to Codex.
use super::paths::{find_binary, home_dir};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const ID: &str = "amplihack@amplihack-local";
#[derive(Serialize, Deserialize)]
struct Ownership {
    schema_version: u32,
    codex_home: PathBuf,
    package_digest: String,
    hooks: Value,
}
fn root() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack/codex"))
}
fn codex_home() -> Result<PathBuf> {
    Ok(std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".codex")))
}
fn regular_json(path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "refusing nonregular Codex JSON file"
            );
            Ok(Some(serde_json::from_slice(&fs::read(path)?).context(
                "malformed Codex JSON; repair manually before retrying",
            )?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn atomic_json(path: &Path, value: &Value, original: Option<Value>) -> Result<()> {
    let parent = path.parent().context("JSON destination has no parent")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if original.is_some() {
        staged
            .as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    staged.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    staged.write_all(b"\n")?;
    staged.as_file().sync_all()?;
    ensure!(
        regular_json(path)? == original,
        "Codex JSON changed concurrently; retry"
    );
    staged
        .persist(path)
        .context("failed atomic Codex JSON write")?;
    Ok(())
}
fn native(binary: &Path, args: &[&str], home: &Path) -> Result<Value> {
    let mut cmd = Command::new(binary);
    cmd.args(args).env("CODEX_HOME", home);
    if !args.contains(&"--json") {
        cmd.arg("--json");
    }
    let output =
        crate::util::run_output_with_timeout_limited(cmd, Duration::from_secs(30), 1024 * 1024)?;
    ensure!(
        output.status.success(),
        "Codex native plugin command {:?} failed ({}): {}",
        args,
        output.status,
        crate::util::format_output_diagnostics(&output, 2048)
    );
    serde_json::from_slice(&output.stdout).context("invalid native Codex plugin response")
}
fn installed(value: &Value) -> bool {
    value["installed"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|p| p["pluginId"].as_str() == Some(ID) && p["installed"].as_bool() == Some(true))
    })
}
fn verify_identity(list: &Value, expected: &Path) -> Result<()> {
    for plugin in list["installed"]
        .as_array()
        .context("native installed inventory must be an array")?
    {
        if plugin["pluginId"].as_str() == Some(ID) {
            let source = plugin
                .pointer("/source/path")
                .and_then(Value::as_str)
                .context("native Codex identity lacks source provenance")?;
            ensure!(
                Path::new(source).canonicalize()? == expected.canonicalize()?,
                "Codex plugin identity belongs to a different source; refusing replacement/removal"
            );
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path, boundary: &Path, depth: usize) -> Result<()> {
    ensure!(depth < 32, "Codex resource nesting exceeds safe depth");
    fs::create_dir_all(destination)?;
    for e in fs::read_dir(source)? {
        let e = e?;
        let target = destination.join(e.file_name());
        let kind = e.file_type()?;
        if kind.is_symlink() {
            let link = fs::read_link(e.path())?;
            ensure!(
                !link.is_absolute(),
                "absolute skill resource symlink is not portable"
            );
            // Keep relative resource links (including intentionally dangling ones)
            // only if lexical resolution stays inside the complete canonical tree.
            let mut resolved = e.path().parent().context("resource parent")?.to_path_buf();
            for c in link.components() {
                match c {
                    std::path::Component::ParentDir => {
                        ensure!(resolved.pop(), "resource symlink escape");
                    }
                    std::path::Component::Normal(n) => resolved.push(n),
                    std::path::Component::CurDir => {}
                    _ => bail!("resource symlink escape"),
                }
            }
            ensure!(
                resolved.starts_with(boundary),
                "skill resource symlink escapes canonical tree"
            );
            if e.path().exists() {
                ensure!(
                    e.path().canonicalize()?.starts_with(boundary),
                    "resolved skill resource escapes canonical tree"
                );
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, target)?;
            #[cfg(not(unix))]
            bail!("symlink resource packaging requires Unix");
        } else if kind.is_dir() {
            copy_tree(&e.path(), &target, boundary, depth + 1)?;
        } else {
            ensure!(kind.is_file(), "unsupported skill resource type");
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}
fn digest(root: &Path) -> Result<String> {
    fn walk(path: &Path, base: &Path, hash: &mut Sha256) -> Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            hash.update(e.path().strip_prefix(base)?.as_os_str().as_encoded_bytes());
            let kind = e.file_type()?;
            if kind.is_symlink() {
                hash.update(b"link");
                hash.update(fs::read_link(e.path())?.as_os_str().as_encoded_bytes());
            } else if kind.is_dir() {
                hash.update(b"dir");
                walk(&e.path(), base, hash)?;
            } else {
                hash.update(b"file");
                hash.update(fs::read(e.path())?);
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    walk(root, root, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn validate_skill_names(
    directory: &Path,
    names: &mut std::collections::BTreeSet<String>,
    depth: usize,
) -> Result<()> {
    ensure!(depth < 32, "skill validation depth exceeds safe limit");
    let skill = directory.join("SKILL.md");
    if skill.is_file() {
        let text = fs::read_to_string(&skill)?;
        let mut lines = text.lines();
        ensure!(
            lines.next() == Some("---"),
            "Codex skill lacks YAML frontmatter: {}",
            skill.display()
        );
        let mut yaml = Vec::new();
        let mut closed = false;
        for line in lines {
            if line.trim() == "---" {
                closed = true;
                break;
            }
            yaml.push(line);
        }
        ensure!(closed, "Codex skill frontmatter is unterminated");
        let frontmatter: serde_yaml::Value = serde_yaml::from_str(&yaml.join("\n"))?;
        let name = frontmatter["name"]
            .as_str()
            .context("Codex skill requires a string name")?;
        ensure!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "invalid canonical Codex skill name"
        );
        ensure!(
            frontmatter["description"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
            "Codex skill requires a nonempty description"
        );
        ensure!(
            names.insert(name.to_string()),
            "duplicate canonical Codex skill identity: {name}"
        );
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            validate_skill_names(&entry.path(), names, depth + 1)?;
        }
    }
    Ok(())
}

// Native plugin discovery stops descending once a parent is itself a skill.
// Expose nested canonical roots through top-level runtime copies, retaining
// their original location and every sibling resource byte.
fn expose_nested_skills(directory: &Path, root: &Path, depth: usize) -> Result<()> {
    ensure!(depth < 32, "nested skill depth exceeds safe limit");
    let entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("SKILL.md").is_file() && path.parent() != Some(root) {
            let alias = root.join(entry.file_name());
            ensure!(!alias.exists(), "nested Codex skill alias collision");
            copy_tree(&path, &alias, root, 0)?;
        }
        expose_nested_skills(&path, root, depth + 1)?;
    }
    Ok(())
}

fn instruction_skills(source: &Path, output: &Path, prefix: &str) -> Result<()> {
    instruction_skills_under(source, source, output, prefix)
}
fn instruction_skills_under(source: &Path, base: &Path, output: &Path, prefix: &str) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for e in fs::read_dir(source)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            instruction_skills_under(&e.path(), base, output, prefix)?;
            continue;
        }
        if e.path().extension().is_none_or(|x| x != "md") {
            continue;
        }
        let stem = e
            .path()
            .strip_prefix(base)?
            .with_extension("")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("-");
        ensure!(
            stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "invalid instruction skill name"
        );
        let name = format!("amplihack-{prefix}-{stem}");
        let dir = output.join(&name);
        ensure!(!dir.exists(), "generated Codex skill collision: {name}");
        fs::create_dir(&dir)?;
        let body = fs::read_to_string(e.path())?;
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Amplihack {prefix} instructions for {stem}\n---\n\n{body}"
            ),
        )?;
    }
    Ok(())
}
fn reconcile_hooks(home: &Path, previous: &Value, desired: &Value) -> Result<()> {
    fs::create_dir_all(home)?;
    ensure!(
        !fs::symlink_metadata(home)?.file_type().is_symlink(),
        "refusing symlink Codex home"
    );
    let lock = fs::File::open(home)?;
    lock.lock_exclusive()?;
    let path = home.join("hooks.json");
    let original = regular_json(&path)?;
    let mut value = original.clone().unwrap_or_else(|| json!({"hooks":{}}));
    ensure!(value.is_object(), "Codex hooks.json must be an object");
    if value.get("hooks").is_none() {
        value["hooks"] = json!({});
    }
    let map = value["hooks"]
        .as_object_mut()
        .context("Codex hooks must be an object")?;
    for (event, owned) in previous.as_object().context("invalid owned hook record")? {
        if let Some(entries) = map.get_mut(event) {
            let entries = entries
                .as_array_mut()
                .context("Codex event handlers must be arrays")?;
            entries.retain(|entry| !owned.as_array().is_some_and(|a| a.contains(entry)));
        }
    }
    for (event, handlers) in desired.as_object().context("invalid desired hooks")? {
        let entries = map
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Codex event handlers must be arrays")?;
        for handler in handlers.as_array().context("invalid hook handlers")? {
            ensure!(
                !entries.contains(handler),
                "identical unowned Codex hook definition exists; preserve it and reconcile ownership manually"
            );
            entries.push(handler.clone());
        }
    }
    atomic_json(&path, &value, original)
}

pub(super) fn install(source: &Path, hooks_binary: &Path) -> Result<bool> {
    let Some(binary) = find_binary("codex") else {
        return Ok(false);
    };
    let root = root()?;
    fs::create_dir_all(&root)?;
    ensure!(
        !fs::symlink_metadata(&root)?.file_type().is_symlink(),
        "refusing symlink Codex staging root"
    );
    let lock = fs::File::open(&root)?;
    lock.lock_exclusive()?;
    let home = codex_home()?;
    fs::create_dir_all(&home)?;
    amplihack_launcher::codex_config::configure(&home, false)?;
    let ledger = root.join("ownership.json");
    let previous = regular_json(&ledger)?
        .map(serde_json::from_value::<Ownership>)
        .transpose()?;
    if let Some(old) = &previous {
        ensure!(
            old.schema_version == 1 && old.codex_home == home,
            "Codex ownership scope changed; uninstall previous scope first"
        );
    }
    let package = root.join("market/plugin");
    let inventory = native(&binary, &["plugin", "list", "--json"], &home)?;
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
    let staged = tempfile::tempdir_in(&root)?;
    let skills = staged.path().join("skills");
    let canonical = source.join("amplifier-bundle/skills").canonicalize()?;
    validate_skill_names(&canonical, &mut std::collections::BTreeSet::new(), 0)?;
    copy_tree(&canonical, &skills, &canonical, 0)?;
    expose_nested_skills(&skills, &skills, 0)?;
    instruction_skills(
        &source.join("docs/claude/commands/amplihack"),
        &skills,
        "command",
    )?;
    instruction_skills(&source.join("amplifier-bundle/agents"), &skills, "persona")?;
    fs::create_dir(staged.path().join("hooks"))?;
    // Empty declaration avoids duplicate execution if bundled hooks become supported.
    fs::write(staged.path().join("hooks/hooks.json"), "{\"hooks\":{}}\n")?;
    fs::write(
        staged.path().join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name":"amplihack", "version":env!("CARGO_PKG_VERSION"),
            "description":"Provider-neutral Amplihack skills and reusable instructions",
            "extensions":{"com.openai":{"hooks":"./hooks/hooks.json"}}
        }))?,
    )?;
    fs::create_dir(staged.path().join("bin"))?;
    let wrapper = staged.path().join("bin/hook");
    // Copy the durable binary into the portable package; shell text has no path interpolation.
    fs::copy(hooks_binary, staged.path().join("bin/amplihack-hooks"))?;
    fs::write(
        &wrapper,
        "#!/bin/sh\nexport AMPLIHACK_AGENT_BINARY=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    }
    let mut hooks = json!({});
    let installed_wrapper = package.join("bin/hook");
    let quoted = format!(
        "'{}'",
        installed_wrapper.to_string_lossy().replace('\'', "'\\''")
    );
    for (event, subcommand) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt"),
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
        ("Stop", "stop"),
        ("SessionEnd", "session-stop"),
    ] {
        hooks[event] =
            json!([{"hooks":[{"type":"command","command":format!("{quoted} {subcommand}")}]}]);
    }
    hooks["UserPromptSubmit"][0]["hooks"].as_array_mut().context("user hook array")?
        .push(json!({"type":"command","command":format!("{quoted} workflow-classification-reminder")}));
    let market = root.join("market");
    fs::create_dir_all(market.join(".agents/plugins"))?;
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
    entries.retain(|entry| entry["name"] != "amplihack");
    entries.push(json!({"name":"amplihack", "source":{"source":"local","path":"./plugin"},
        "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"}, "category":"Productivity"}));
    atomic_json(&marketplace_path, &marketplace, original_marketplace)?;

    if package.exists() {
        fs::remove_dir_all(&package)?;
    }
    fs::rename(staged.path(), &package)?;
    // Write ownership before external registration so partial installs remain removable.
    let mut recoverable_hooks = hooks.clone();
    if let Some(old) = &previous {
        for (event, entries) in old
            .hooks
            .as_object()
            .context("invalid previous hook ownership")?
        {
            let owned = recoverable_hooks
                .as_object_mut()
                .context("invalid desired hooks")?
                .entry(event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("owned hooks must be arrays")?;
            for entry in entries
                .as_array()
                .context("previous owned hooks must be arrays")?
            {
                if !owned.contains(entry) {
                    owned.push(entry.clone());
                }
            }
        }
    }
    let mut record = Ownership {
        schema_version: 1,
        codex_home: home.clone(),
        package_digest: digest(&package)?,
        hooks: recoverable_hooks,
    };
    let original = regular_json(&ledger)?;
    atomic_json(&ledger, &serde_json::to_value(&record)?, original)?;
    native(
        &binary,
        &[
            "plugin",
            "marketplace",
            "add",
            market
                .to_str()
                .context("Codex marketplace path must be UTF-8")?,
        ],
        &home,
    )?;
    native(&binary, &["plugin", "add", ID], &home)?;
    ensure!(
        installed(&native(&binary, &["plugin", "list", "--json"], &home)?),
        "native Codex registration missing installed identity"
    );
    reconcile_hooks(
        &home,
        &previous.map(|p| p.hooks).unwrap_or_else(|| json!({})),
        &hooks,
    )?;
    record.hooks = hooks;
    let original = regular_json(&ledger)?;
    atomic_json(&ledger, &serde_json::to_value(&record)?, original)?;
    println!(
        "  ✅ Native Codex plugin installed; review exact hooks in Codex /hooks before trusting them"
    );
    Ok(true)
}

pub(super) fn uninstall() -> Result<()> {
    let root = root()?;
    if !root.exists() {
        return Ok(());
    }
    ensure!(
        !fs::symlink_metadata(&root)?.file_type().is_symlink(),
        "refusing symlink Codex staging root"
    );
    let lock = fs::File::open(&root)?;
    lock.lock_exclusive()?;
    let Some(record) = regular_json(&root.join("ownership.json"))? else {
        return Ok(());
    };
    let record: Ownership = serde_json::from_value(record)?;
    ensure!(
        record.schema_version == 1,
        "unknown Codex ownership version; refusing removal"
    );
    let binary = find_binary("codex")
        .context("Codex required to unregister owned plugin before deletion")?;
    let inventory = native(&binary, &["plugin", "list", "--json"], &record.codex_home)?;
    verify_identity(&inventory, &root.join("market/plugin"))?;
    if installed(&inventory) {
        native(&binary, &["plugin", "remove", ID], &record.codex_home)?;
        ensure!(
            !installed(&native(
                &binary,
                &["plugin", "list", "--json"],
                &record.codex_home
            )?),
            "Codex plugin remains installed; retaining package"
        );
    }
    reconcile_hooks(&record.codex_home, &record.hooks, &json!({}))?;
    let package = root.join("market/plugin");
    if package.exists() && digest(&package)? == record.package_digest {
        fs::remove_dir_all(package)?;
    } else if package.exists() {
        bail!("Codex package modified; preserving package and ownership record");
    }
    fs::remove_file(root.join("ownership.json"))?;
    // Marketplace retained: it may have acquired foreign entries.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_hooks_reconcile_preserves_foreign_definitions_across_update_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let foreign =
            json!({"matcher":"foreign","hooks":[{"type":"command","command":"echo user-owned"}]});
        let initial = json!({"user_metadata":{"retain":true},"hooks":{"SessionStart":[foreign]}});
        fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
        let owned = json!({"SessionStart":[{"hooks":[{"type":"command","command":"amplihack-managed-hook"}]}]});
        reconcile_hooks(dir.path(), &json!({}), &owned).unwrap();
        reconcile_hooks(dir.path(), &owned, &owned).unwrap();
        assert_eq!(
            regular_json(&path).unwrap().unwrap()["hooks"]["SessionStart"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        reconcile_hooks(dir.path(), &owned, &json!({})).unwrap();
        assert_eq!(regular_json(&path).unwrap().unwrap(), initial);
    }

    #[test]
    fn codex_hooks_malformed_json_and_unowned_collisions_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(&path, "{broken").unwrap();
        assert!(reconcile_hooks(dir.path(), &json!({}), &json!({})).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
        let owned = json!({"SessionStart":[{"hooks":[{"type":"command","command":"identical"}]}]});
        let original = json!({"hooks":owned});
        fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        assert!(reconcile_hooks(dir.path(), &json!({}), &owned).is_err());
        assert_eq!(regular_json(&path).unwrap().unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn codex_resource_and_config_symlink_escape_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let staged = dir.path().join("staged");
        fs::create_dir(&source).unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "foreign").unwrap();
        std::os::unix::fs::symlink("../outside", source.join("resource")).unwrap();
        assert!(copy_tree(&source, &staged, &source, 0).is_err());
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("hooks.json")).unwrap();
        assert!(reconcile_hooks(&home, &json!({}), &json!({})).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "foreign");
    }
}
