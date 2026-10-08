//! Regular-file snapshots and atomic writes.
use super::*;
pub(super) fn regular_json(path: &Path) -> Result<Option<Value>> {
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
pub(super) fn atomic_json(path: &Path, value: &Value, original: Option<Value>) -> Result<()> {
    path_scope::check()?;
    let parent = path.parent().context("JSON destination has no parent")?;
    fs::create_dir_all(parent)?;
    path_scope::check()?;
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
    path_scope::check()?;
    staged
        .persist(path)
        .context("failed atomic Codex JSON write")?;
    sync_path(parent)?;
    Ok(())
}

/// Snapshot absence or exact regular-file bytes before live mutations.
pub(super) fn snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "refusing nonregular Codex snapshot"
            );
            Ok(Some(fs::read(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
pub(super) fn restore_bytes(path: &Path, value: &Value, expected: &Value) -> Result<()> {
    path_scope::check()?;
    let current = snapshot(path)?;
    let state = serde_json::to_value(&current)?;
    ensure!(
        &state == value || &state == expected,
        "foreign Codex snapshot changed; reconcile manually; recovery record retained"
    );
    if value.is_null() {
        if current.is_some() {
            ensure!(
                snapshot(path)? == current,
                "Codex snapshot changed concurrently"
            );
            path_scope::check()?;
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    let bytes: Vec<u8> = value
        .as_array()
        .context("invalid recovery snapshot")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .context("invalid recovery byte")
        })
        .collect::<Result<_>>()?;
    let parent = path.parent().context("snapshot parent missing")?;
    fs::create_dir_all(parent)?;
    path_scope::check()?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if current.is_some() {
        staged
            .as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    ensure!(
        snapshot(path)? == current,
        "Codex snapshot changed concurrently"
    );
    path_scope::check()?;
    staged.persist(path).context("snapshot recovery failed")?;
    Ok(())
}

/// The exact serialization used by atomic_json and recorded before live writes.
pub(super) fn json_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Sync a validated resource without following symlinks.
pub(super) fn sync_path(path: &Path) -> Result<()> {
    let canonical = path_scope::canonical(path)?;
    let path = canonical.as_path();
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        !meta.file_type().is_symlink(),
        "refusing symlink durability resource"
    );
    #[cfg(test)]
    testing::check(path)?;
    path_scope::check()?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)?
        .sync_all()
        .context("Codex resource synchronization failed")?;
    path_scope::check()
}

pub(super) fn sync_tree(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    // Portable resources intentionally include relative and dangling links.
    // Their entries are persisted by syncing the containing directory; never
    // follow them or synchronize resources outside the managed tree.
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    ensure!(
        meta.is_dir() || meta.is_file(),
        "unsupported durability resource type"
    );
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
    }
    sync_path(path)
}

pub(super) fn sync_dependencies(root: &Path, home: &Path) -> Result<()> {
    sync_tree(&root.join("market"))?;
    for name in ["config.toml", "hooks.json"] {
        let path = home.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => sync_path(&path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    sync_path(home)?;
    sync_path(home.parent().context("Codex home parent missing")?)?;
    sync_path(root)?;
    sync_path(root.parent().context("Codex staging parent missing")?)
}

#[cfg(test)]
pub(super) mod testing {
    use super::*;
    type Hook = dyn FnMut(&Path) -> Result<()>;
    thread_local! { static HOOK: std::cell::RefCell<Option<*mut Hook>> = std::cell::RefCell::new(None); }
    pub fn with_sync_hook<R>(
        mut hook: impl FnMut(&Path) -> Result<()>,
        run: impl FnOnce() -> R,
    ) -> R {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                HOOK.with(|h| *h.borrow_mut() = None);
            }
        }
        let borrowed: &mut (dyn FnMut(&Path) -> Result<()> + '_) = &mut hook;
        // The pointer is thread-local and is cleared by the guard before the scoped callback dies,
        // including on unwinding. No callback reference escapes this synchronous scope.
        let pointer: *mut Hook = unsafe { std::mem::transmute(borrowed) };
        HOOK.with(|h| {
            assert!(h.borrow().is_none());
            *h.borrow_mut() = Some(pointer);
        });
        let _reset = Reset;
        run()
    }
    pub(super) fn check(path: &Path) -> Result<()> {
        HOOK.with(|h| {
            // Keep exclusive access across invocation: reentry must fail before
            // constructing another mutable reference to the scoped callback.
            let hook = h.borrow_mut();
            match *hook {
                Some(ptr) => unsafe { (&mut *ptr)(path) },
                None => Ok(()),
            }
        })
    }
}
