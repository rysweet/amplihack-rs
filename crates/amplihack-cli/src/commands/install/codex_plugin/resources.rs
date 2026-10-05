//! Canonical resource validation, copying and digest identity.
use super::*;
pub(super) fn copy_tree(
    source: &Path,
    destination: &Path,
    boundary: &Path,
    depth: usize,
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
            copy_tree(&e.path(), &target, boundary, depth + 1)?;
        } else {
            ensure!(kind.is_file(), "unsupported skill resource type");
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}
pub(super) fn digest(root: &Path) -> Result<String> {
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
pub(super) fn validate_skill_names(
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
pub(super) fn expose_nested_skills(directory: &Path, root: &Path, depth: usize) -> Result<()> {
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
