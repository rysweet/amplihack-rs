//! Sets `<root>/.default` directly, as `profile switch NAME` did before
//! issue #1691. Since #1691 `switch NAME` also moves the live login and pins
//! rotation, so a test that only needs a default pointer writes it here.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Writes `<name>\n` to `<root>/.default`, mode 0600 on Unix, the way
/// `pointer::write` does. The profile directory must already exist.
pub fn set_default(root: &Path, name: &str) {
    assert!(
        root.join(name).is_dir(),
        "profile {name} must exist before it is made the default"
    );
    let path = root.join(".default");
    fs::write(&path, format!("{name}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
