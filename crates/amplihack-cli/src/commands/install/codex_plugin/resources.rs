//! Canonical resource validation, copying and digest identity.
use super::*;
/// Full-list packages retain Codex's existing analyst exclusion (#3195).
/// Default packages allow explicitly enabled analysts like any other skill.
pub(super) fn left_out_of_codex(directory: &Path) -> bool {
    directory.join("SKILL.md").is_file()
        && directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("-analyst"))
}
pub(super) fn copy_tree(
    source: &Path,
    destination: &Path,
    boundary: &Path,
    depth: usize,
    selection: &ListSelection,
) -> Result<()> {
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
            if selection.set == ListSet::All && left_out_of_codex(&e.path()) {
                continue;
            }
            copy_tree(&e.path(), &target, boundary, depth + 1, selection)?;
        } else {
            ensure!(kind.is_file(), "unsupported skill resource type");
            // Preserve neighbouring resources and relative links without making
            // unselected folders discoverable as skills.
            if e.file_name() == "SKILL.md"
                && !source
                    .file_name()
                    .is_some_and(|name| selection.includes_skill(&name.to_string_lossy()))
            {
                continue;
            }
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}
pub(super) fn digest(root: &Path) -> Result<String> {
    #[cfg(test)]
    super::cleanup_performance_tests::DIGESTS.with(|n| n.set(n.get() + 1));
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    fn walk(path: &Path, base: &Path, hash: &mut Sha256) -> Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            field(
                hash,
                e.path().strip_prefix(base)?.as_os_str().as_encoded_bytes(),
            );
            let kind = e.file_type()?;
            if kind.is_symlink() {
                hash.update(b"L");
                field(
                    hash,
                    fs::read_link(e.path())?.as_os_str().as_encoded_bytes(),
                );
            } else if kind.is_dir() {
                hash.update(b"D");
                walk(&e.path(), base, hash)?;
            } else {
                ensure!(kind.is_file(), "unsupported Codex digest entry type");
                hash.update(b"F");
                let bytes = fs::read(e.path())?;
                #[cfg(test)]
                super::cleanup_performance_tests::HASHED_BYTES
                    .with(|n| n.set(n.get() + bytes.len() as u64));
                field(hash, &bytes);
            }
        }
        Ok(())
    }
    ensure!(
        fs::symlink_metadata(root)?.is_dir()
            && !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "nonregular Codex digest root"
    );
    let mut hash = Sha256::new();
    hash.update(b"amplihack-codex-tree-v2\0");
    walk(root, root, &mut hash)?;
    Ok(format!("v2:{:x}", hash.finalize()))
}
/// Pre-framing tree identity, used only to verify a prior ledger before migrating it.
pub(super) fn legacy_digest(root: &Path) -> Result<String> {
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
                ensure!(kind.is_file(), "unsupported Codex digest entry type");
                hash.update(b"file");
                hash.update(fs::read(e.path())?);
            }
        }
        Ok(())
    }
    let meta = fs::symlink_metadata(root)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "nonregular Codex digest root"
    );
    let mut hash = Sha256::new();
    walk(root, root, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}
pub(super) fn legacy_digest_format(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
/// Legacy unframed identities cannot authorize adoption, replacement or cleanup.
pub(super) fn current_digest(value: &str) -> bool {
    value
        .strip_prefix("v2:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(super) fn validate_skill_names(
    directory: &Path,
    names: &mut std::collections::BTreeSet<String>,
    depth: usize,
    selection: &ListSelection,
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
        if entry.file_type()?.is_dir()
            && !(selection.set == ListSet::All && left_out_of_codex(&entry.path()))
        {
            validate_skill_names(&entry.path(), names, depth + 1, selection)?;
        }
    }
    Ok(())
}

// Native plugin discovery stops descending once a parent is itself a skill.
// Expose nested canonical roots through top-level runtime copies, retaining
// their original location and every sibling resource byte.
pub(super) fn expose_nested_skills(
    directory: &Path,
    root: &Path,
    depth: usize,
    selection: &ListSelection,
) -> Result<()> {
    ensure!(depth < 32, "nested skill depth exceeds safe limit");
    let entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let nested = path.join("SKILL.md").is_file() && path.parent() != Some(root);
        if nested {
            let alias = root.join(entry.file_name());
            ensure!(!alias.exists(), "nested Codex skill alias collision");
            copy_tree(&path, &alias, root, 0, selection)?;
        }
        expose_nested_skills(&path, root, depth + 1, selection)?;
        if nested && selection.set == ListSet::Default {
            // The top-level publication is the sole discoverable definition.
            // Keep the original resource location for relative references.
            fs::remove_file(path.join("SKILL.md"))?;
        }
    }
    Ok(())
}
