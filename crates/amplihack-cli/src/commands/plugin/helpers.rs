use super::*;

pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    let src_root = src
        .canonicalize()
        .with_context(|| format!("failed to resolve source root {}", src.display()))?;
    copy_dir_recursive_inner(src, dst, &src_root)
}

fn copy_dir_recursive_inner(src: &Path, dst: &Path, src_root: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir_recursive_inner(&source, &target, src_root)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        } else if kind.is_symlink() {
            let link_target = fs::read_link(&source)
                .with_context(|| format!("failed to read {}", source.display()))?;

            // SEC-3: Validate that the symlink target does not escape the plugin
            // source directory.
            if link_target.is_absolute() {
                anyhow::bail!(
                    "plugin contains a symlink with an absolute target, which is not allowed: \
                    {} -> {}",
                    source.display(),
                    link_target.display()
                );
            }

            let resolved = source.parent().unwrap_or(src).join(&link_target);
            let normalized = normalize_path(&resolved);
            if !normalized.starts_with(src_root) {
                anyhow::bail!(
                    "plugin contains a symlink that escapes the plugin directory (path traversal \
                    attack): {} -> {} (resolves to {})",
                    source.display(),
                    link_target.display(),
                    normalized.display()
                );
            }

            create_symlink(&link_target, &target)?;
        }
    }
    Ok(())
}

/// Normalize a path lexically (remove `.` and `..` components) without
/// requiring the path to exist on disk.
fn normalize_path(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut components: Vec<_> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if components.last() == Some(&Component::Normal(std::ffi::OsStr::new(".."))) {
                    components.push(component);
                } else {
                    components.pop();
                }
            }
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;
    Ok(())
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    let metadata = fs::metadata(target)
        .with_context(|| format!("failed to stat symlink target {}", target.display()))?;
    if metadata.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| format!("failed to create symlink {}", link.display()))?;
    Ok(())
}

pub(super) fn default_plugin_root() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("plugins"))
}

pub(super) fn plugins_json_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".config")
        .join("claude-code")
        .join("plugins.json"))
}

pub(super) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

pub(super) fn is_path_safe(path: &Path, base: &Path) -> Result<bool> {
    let resolved_base = base
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", base.display()))?;
    let resolved_path = path
        .parent()
        .unwrap_or(base)
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    Ok(resolved_path.starts_with(&resolved_base))
}

pub(super) fn plugin_name_from_git_url(source: &str) -> Result<String> {
    let mut name = source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    if let Some(stripped) = name.strip_suffix(".git") {
        name = stripped.to_string();
    }
    Ok(name)
}

pub(super) fn is_valid_semver(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), Some(c), None)
            if !a.is_empty()
                && !b.is_empty()
                && !c.is_empty()
                && a.chars().all(|ch| ch.is_ascii_digit())
                && b.chars().all(|ch| ch.is_ascii_digit())
                && c.chars().all(|ch| ch.is_ascii_digit())
    )
}

pub(super) fn is_valid_plugin_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}
