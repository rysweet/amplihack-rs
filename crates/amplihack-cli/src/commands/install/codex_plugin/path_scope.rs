//! Invocation-local directory identities and lexical mappings. Resource links
//! remain entries; pathname checks do not promise syscall-level race exclusion.
use super::*;
use std::{cell::RefCell, collections::BTreeMap, path::Component};

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
fn directory(path: &Path) -> Result<Option<Identity>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "foreign Codex directory component; record retained"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(Identity {
            device: meta.dev(),
            inode: meta.ino(),
        }))
    }
    #[cfg(not(unix))]
    Ok(Some(Identity {}))
}
fn absolute(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "Codex path contains escaping parent component; record retained"
    );
    Ok(if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir()?.join(path)
    })
}

struct Anchor {
    lexical: PathBuf,
    canonical: PathBuf,
    identity: Identity,
}
pub(super) struct InstallerPathScope {
    cwd: PathBuf,
    root: PathBuf,
    home: PathBuf,
    anchors: Vec<Anchor>,
    directories: BTreeMap<PathBuf, Identity>,
}
impl InstallerPathScope {
    fn capture(root: &Path, home: &Path) -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let root = absolute(root)?;
        let home = absolute(home)?;
        let selected = absolute(&home_dir()?)?;
        let selected_staging = root.starts_with(&selected);
        if selected_staging {
            directory(&selected)?.context("selected HOME must be a present regular directory")?;
        }
        directory(&home)?;
        directory(home.parent().context("Codex home parent missing")?)?;
        let mut boundaries = Vec::new();
        if selected_staging {
            boundaries.push(selected.clone());
        }
        // Production staging is beneath selected HOME. Direct recovery also
        // accepts explicit staging paths; freeze their volume entry rather
        // than walking or synchronizing the filesystem root.
        if !root.starts_with(&selected) {
            boundaries.push(
                root.ancestors()
                    .find(|p| p.parent().is_some_and(|a| a.parent().is_none()))
                    .context("Codex staging path lacks bounded ancestor")?
                    .to_path_buf(),
            );
        }
        let mut parent = home.parent().context("Codex home parent missing")?;
        while directory(parent)?.is_none() {
            parent = parent
                .parent()
                .context("Codex home has no existing anchor")?;
        }
        boundaries.push(parent.into());
        let mut scope = Self {
            cwd,
            root,
            home,
            anchors: Vec::new(),
            directories: BTreeMap::new(),
        };
        for lexical in boundaries {
            let identity = directory(&lexical)?.context("Codex caller anchor missing")?;
            let canonical = fs::canonicalize(&lexical)?;
            scope.anchors.push(Anchor {
                lexical,
                canonical,
                identity,
            });
        }
        scope.revalidate()?;
        Ok(scope)
    }
    fn paths(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.home.clone(),
            self.root.join("market/.agents/plugins"),
        ]
    }
    fn boundary(&self, path: &Path) -> Result<&Anchor> {
        // A CODEX_HOME parent may also contain an explicit recovery staging
        // root. It must not shorten that root's captured publication scope.
        if path.starts_with(&self.root) || self.root.starts_with(path) {
            return self
                .anchors
                .iter()
                .take(self.anchors.len() - 1)
                .filter(|a| path.starts_with(&a.lexical))
                .max_by_key(|a| a.lexical.components().count())
                .context("Codex staging path outside captured anchors");
        }
        self.anchors
            .iter()
            .filter(|a| path.starts_with(&a.lexical))
            .max_by_key(|a| a.lexical.components().count())
            .context("Codex path outside captured caller anchors")
    }
    fn revalidate(&mut self) -> Result<()> {
        ensure!(
            std::env::current_dir()? == self.cwd,
            "Codex invocation working directory changed; record retained"
        );
        for anchor in &self.anchors {
            ensure!(
                directory(&anchor.lexical)?.as_ref() == Some(&anchor.identity)
                    && fs::canonicalize(&anchor.lexical)? == anchor.canonical,
                "captured Codex anchor or alias changed; record retained"
            );
        }
        for (path, identity) in &self.directories {
            ensure!(
                directory(path)?.as_ref() == Some(identity),
                "captured Codex directory replaced; record retained"
            );
        }
        // Adopt only previously missing regular descendants of unchanged anchors.
        for path in self.paths() {
            let anchor = self.boundary(&path)?.lexical.clone();
            for ancestor in path
                .ancestors()
                .take_while(|p| p.starts_with(&anchor))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                if let Some(identity) = directory(ancestor)? {
                    self.directories.entry(ancestor.into()).or_insert(identity);
                }
            }
        }
        Ok(())
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf> {
        let path = absolute(path)?;
        if let Ok(anchor) = self.boundary(&path) {
            return Ok(anchor.canonical.join(path.strip_prefix(&anchor.lexical)?));
        }
        ensure!(
            self.anchors.iter().any(|a| path.starts_with(&a.canonical)),
            "Codex synchronization path outside captured anchors"
        );
        Ok(path)
    }
    fn ancestors(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let path = absolute(path)?;
        let anchor = self.boundary(&path)?;
        let mut result = Vec::new();
        for ancestor in path
            .ancestors()
            .take_while(|p| p.starts_with(&anchor.lexical))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if directory(ancestor)?.is_some() {
                result.push(self.canonical(ancestor)?);
            }
        }
        Ok(result)
    }
}

// Synchronous install/uninstall/recovery boundaries carry ONE captured scope.
// Native/storage helpers consult it so no inner recovery can recapture a
// replacement. The guard clears state even on unwind; no pointer escapes.
thread_local! { static ACTIVE: RefCell<Option<InstallerPathScope>> = const { RefCell::new(None) }; }
pub(super) fn with_scope<T>(
    root: &Path,
    home: &Path,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if ACTIVE.with(|s| s.borrow().is_some()) {
        check()?;
        ACTIVE.with(|s| {
            let s = s.borrow();
            let scope = s.as_ref().context("Codex scope missing")?;
            ensure!(
                absolute(root)? == scope.root && absolute(home)? == scope.home,
                "nested Codex recovery changed captured scope"
            );
            Ok(())
        })?;
        return action();
    }
    let scope = InstallerPathScope::capture(root, home)?;
    ACTIVE.with(|s| *s.borrow_mut() = Some(scope));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|s| *s.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    let value = action()?;
    check()?;
    Ok(value)
}
pub(super) fn check() -> Result<()> {
    ACTIVE.with(|s| match s.borrow_mut().as_mut() {
        Some(scope) => scope.revalidate(),
        None => Ok(()),
    })
}
pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    check()?;
    ACTIVE.with(|s| match s.borrow().as_ref() {
        Some(scope) => scope.canonical(path),
        None => Ok(path.into()),
    })
}
pub(super) fn ancestors(path: &Path) -> Result<Vec<PathBuf>> {
    check()?;
    ACTIVE.with(|s| {
        s.borrow()
            .as_ref()
            .context("Codex synchronization scope missing")?
            .ancestors(path)
    })
}
