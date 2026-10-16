//! Portable skill-resource construction and byte-exact acceptance helpers.
use super::*;

pub(super) fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let destination = to.join(entry.file_name());
        if entry.file_type().unwrap().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), destination).unwrap();
        } else if entry.path().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

pub(super) fn find_package(root: &Path) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if entry.file_type().ok()?.is_dir() {
            if path.join("plugin.json").is_file() && path.join("skills").is_dir() {
                return Some(path);
            }
            if let Some(found) = find_package(&path) {
                return Some(found);
            }
        }
    }
    None
}

pub(super) fn find_resource(root: &Path) -> bool {
    fs::read_dir(root).unwrap().flatten().any(|entry| {
        if entry.path().is_dir() {
            find_resource(&entry.path())
        } else {
            entry.file_name() == "guide.md"
                && fs::read_to_string(entry.path()).unwrap() == "provider-neutral nested resource\n"
        }
    })
}

pub(super) fn skill_roots(root: &Path, found: &mut Vec<std::path::PathBuf>) {
    if root.join("SKILL.md").is_file() {
        found.push(root.to_path_buf());
    }
    for entry in fs::read_dir(root).unwrap().flatten() {
        if entry.path().is_dir() {
            skill_roots(&entry.path(), found);
        }
    }
}

pub(super) fn assert_resource_tree(source: &Path, staged: &Path) {
    for entry in fs::read_dir(source).unwrap().flatten() {
        let destination = staged.join(entry.file_name());
        if entry.file_type().unwrap().is_symlink() && !entry.path().exists() {
            assert_eq!(
                fs::read_link(entry.path()).unwrap(),
                fs::read_link(destination).unwrap()
            );
        } else if entry.path().is_dir() {
            assert_resource_tree(&entry.path(), &destination);
        } else {
            assert_eq!(
                fs::read(entry.path()).unwrap(),
                fs::read(destination).unwrap()
            );
        }
    }
}
