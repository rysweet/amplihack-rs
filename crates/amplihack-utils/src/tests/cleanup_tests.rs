use super::*;
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// CleanupRegistry::new
// ---------------------------------------------------------------------------

#[test]
fn new_creates_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let reg_dir = tmp.path().join("subdir");
    let reg = CleanupRegistry::new(&reg_dir).expect("new");
    assert!(reg_dir.is_dir());
    assert!(reg.get_tracked_paths().is_empty());
}

// ---------------------------------------------------------------------------
// register
// ---------------------------------------------------------------------------

#[test]
fn register_adds_path() {
    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    let file = tmp.path().join("test.txt");
    fs::write(&file, "data").expect("write");

    reg.register(&file).expect("register");
    assert_eq!(reg.get_tracked_paths().len(), 1);
}

#[test]
fn register_deduplicates() {
    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    let file = tmp.path().join("test.txt");
    fs::write(&file, "data").expect("write");

    reg.register(&file).expect("first");
    reg.register(&file).expect("second");
    assert_eq!(reg.get_tracked_paths().len(), 1);
}

#[test]
fn register_rejects_when_full() {
    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    // Fill up with unique paths (they don't need to exist for registration).
    for i in 0..MAX_TRACKED_PATHS {
        reg.tracked_paths.push(PathBuf::from(format!("/fake/{i}")));
    }

    let result = reg.register(Path::new("/fake/overflow"));
    assert!(result.is_err());
    if let Err(CleanupError::RegistryFull { max }) = result {
        assert_eq!(max, MAX_TRACKED_PATHS);
    }
}

// ---------------------------------------------------------------------------
// deletion_order
// ---------------------------------------------------------------------------

#[test]
fn deletion_order_is_deepest_first() {
    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    reg.tracked_paths.push(PathBuf::from("/a"));
    reg.tracked_paths.push(PathBuf::from("/a/b/c"));
    reg.tracked_paths.push(PathBuf::from("/a/b"));

    let order = reg.deletion_order();
    assert_eq!(order[0], PathBuf::from("/a/b/c"));
    assert_eq!(order[1], PathBuf::from("/a/b"));
    assert_eq!(order[2], PathBuf::from("/a"));
}

// ---------------------------------------------------------------------------
// save / load round-trip
// ---------------------------------------------------------------------------

#[test]
fn save_and_load_round_trip() {
    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    let file1 = tmp.path().join("file1.txt");
    let file2 = tmp.path().join("file2.txt");
    fs::write(&file1, "a").expect("write");
    fs::write(&file2, "b").expect("write");

    reg.register(&file1).expect("reg1");
    reg.register(&file2).expect("reg2");
    reg.save().expect("save");

    // Verify file exists.
    assert!(tmp.path().join(REGISTRY_FILENAME).is_file());

    // Load and verify.
    let loaded = CleanupRegistry::load(tmp.path()).expect("load");
    assert_eq!(loaded.get_tracked_paths().len(), 2);
    assert_eq!(loaded.session_id, reg.session_id);
}

#[test]
fn load_nonexistent_returns_empty() {
    let tmp = TempDir::new().expect("tempdir");
    let fresh_dir = tmp.path().join("empty");
    let loaded = CleanupRegistry::load(&fresh_dir).expect("load");
    assert!(loaded.get_tracked_paths().is_empty());
}

#[test]
fn load_malformed_json_returns_empty() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join(REGISTRY_FILENAME), "NOT JSON!!!").expect("write");

    let loaded = CleanupRegistry::load(tmp.path()).expect("load");
    assert!(loaded.get_tracked_paths().is_empty());
}

// ---------------------------------------------------------------------------
// cleanup_all
// ---------------------------------------------------------------------------

#[test]
fn cleanup_removes_files() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).expect("mkdir");

    let mut reg = CleanupRegistry::new(&work).expect("new");

    let f1 = work.join("temp1.txt");
    let f2 = work.join("temp2.txt");
    fs::write(&f1, "a").expect("write");
    fs::write(&f2, "b").expect("write");

    reg.register(&f1).expect("reg");
    reg.register(&f2).expect("reg");

    let cleaned = reg.cleanup_all().expect("cleanup");
    assert_eq!(cleaned, 2);
    assert!(!f1.exists());
    assert!(!f2.exists());
    assert!(reg.get_tracked_paths().is_empty());
}

#[test]
fn cleanup_removes_directories() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).expect("mkdir");

    let mut reg = CleanupRegistry::new(&work).expect("new");

    let subdir = work.join("subdir");
    fs::create_dir_all(&subdir).expect("mkdir");
    fs::write(subdir.join("inner.txt"), "x").expect("write");

    reg.register(&subdir).expect("reg");

    let cleaned = reg.cleanup_all().expect("cleanup");
    assert_eq!(cleaned, 1);
    assert!(!subdir.exists());
}

#[test]
fn cleanup_skips_nonexistent_paths() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).expect("mkdir");

    let mut reg = CleanupRegistry::new(&work).expect("new");
    reg.tracked_paths.push(work.join("does_not_exist.txt"));

    let cleaned = reg.cleanup_all().expect("cleanup");
    assert_eq!(cleaned, 0);
}

#[cfg(unix)]
#[test]
fn cleanup_skips_symlinks() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).expect("mkdir");

    let mut reg = CleanupRegistry::new(&work).expect("new");

    let real = work.join("real.txt");
    fs::write(&real, "data").expect("write");
    let link = work.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    reg.register(&link).expect("reg");

    let cleaned = reg.cleanup_all().expect("cleanup");
    assert_eq!(cleaned, 0, "symlinks should be skipped");
    assert!(real.exists(), "real file should still exist");
}

// ---------------------------------------------------------------------------
// validate_cleanup_path
// ---------------------------------------------------------------------------

#[test]
fn validate_accepts_contained_path() {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("test.txt");
    fs::write(&file, "x").expect("write");

    assert!(validate_cleanup_path(&file, tmp.path()).is_ok());
}

#[test]
fn validate_rejects_escaped_path() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).expect("mkdir");

    // Create a file outside the working directory.
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "x").expect("write");

    assert!(validate_cleanup_path(&outside, &work).is_err());
}

// ---------------------------------------------------------------------------
// save permissions (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn save_sets_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let mut reg = CleanupRegistry::new(tmp.path()).expect("new");

    let file = tmp.path().join("test.txt");
    fs::write(&file, "x").expect("write");
    reg.register(&file).expect("reg");
    reg.save().expect("save");

    let reg_file = tmp.path().join(REGISTRY_FILENAME);
    let mode = fs::metadata(&reg_file).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "registry file should be owner-only");
}
