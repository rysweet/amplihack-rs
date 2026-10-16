//! Portable observation at chmod boundaries before each artifact's first payload.
use super::*;
use std::{
    os::unix::ffi::OsStrExt,
    process::Stdio,
    time::{Duration, Instant},
};

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "permission observer handshake timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn private_modes_before_payload_under_each_child_umask() {
    for mask in ["000", "002", "022", "077"] {
        let f = Fixture::new();
        f.large();
        let root = f.temp.path().join("tmp");
        let ready = root.join("permission-ready");
        let release = root.join("permission-release");
        // Only dispatch is controlled; Rust observes real filesystem metadata.
        // No Linux /proc, stat option or platform-specific device is involved.
        f.shim(
            "chmod",
            r#"/bin/chmod "$@"
printf '%s\0' "$@" > "$TMPDIR/permission-pending"
/bin/mv "$TMPDIR/permission-pending" "$TMPDIR/permission-ready"
while [ ! -e "$TMPDIR/permission-release" ]; do /bin/sleep 0.01; done
/bin/rm "$TMPDIR/permission-ready"
while [ -e "$TMPDIR/permission-release" ]; do /bin/sleep 0.01; done"#,
        );
        let child = f
            .child(&f.shell)
            .args([
                "-c",
                &format!("umask {mask}\n{}", step_command("workflow-pr-review", STEP)),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Owned directory, report, summary, then each of six scan/list pairs.
        for event in 0..9 {
            wait_until(|| ready.exists());
            // Signal is complete once the final NUL has been written.
            wait_until(|| fs::read(&ready).unwrap().last() == Some(&0));
            let args = fs::read(&ready).unwrap();
            for arg in args.split(|b| *b == 0).skip(1).filter(|b| !b.is_empty()) {
                let path = PathBuf::from(std::ffi::OsStr::from_bytes(arg));
                let metadata = fs::symlink_metadata(&path).unwrap();
                assert!(!metadata.file_type().is_symlink());
                assert_eq!(metadata.uid(), fs::metadata(f.temp.path()).unwrap().uid());
                if event == 0 {
                    assert!(metadata.is_dir());
                    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
                    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
                } else {
                    assert!(metadata.is_file());
                    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
                    assert_eq!(metadata.len(), 0, "artifact already contains payload");
                    let parent = fs::symlink_metadata(path.parent().unwrap()).unwrap();
                    assert_eq!(parent.permissions().mode() & 0o777, 0o700);
                }
            }
            fs::write(&release, b"continue").unwrap();
            wait_until(|| !ready.exists());
            fs::remove_file(&release).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let j = receipt(&out);
        let data = report(&f, &j);
        assert_large_parity(&f, &j, &data);
    }
}

#[test]
fn unsafe_allocations_never_touch_foreign_artifacts() {
    let f = Fixture::new();
    let foreign = f.temp.path().join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o755)).unwrap();
    let sentinel = foreign.join("report");
    fs::write(&sentinel, b"foreign\x00\xff\nuntouched").unwrap();
    let link = f.temp.path().join("tmp/step19c.link");
    std::os::unix::fs::symlink(&foreign, &link).unwrap();
    let occupied = f.temp.path().join("tmp/step19c.occupied");
    fs::create_dir(&occupied).unwrap();
    fs::set_permissions(&occupied, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(occupied.join("report"), b"occupied").unwrap();
    f.shim("mktemp", "printf '%s\n' \"$FOREIGN\"");
    for supplied in [&foreign, &link, &occupied] {
        let before = fs::symlink_metadata(supplied).unwrap();
        let out = f
            .child(&f.shell)
            .args(["-c", step_command("workflow-pr-review", STEP)])
            .env("FOREIGN", supplied)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let after = fs::symlink_metadata(supplied).unwrap();
        assert_eq!(
            (before.mode(), before.ctime(), before.ctime_nsec()),
            (after.mode(), after.ctime(), after.ctime_nsec())
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"foreign\x00\xff\nuntouched");
        assert_eq!(fs::read(occupied.join("report")).unwrap(), b"occupied");
    }
    f.shim("mktemp", r#"case "$1" in */scan.*|*/files.*) printf '%s\n' "$FOREIGN";; *) exec /usr/bin/mktemp "$@";; esac"#);
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env("FOREIGN", &sentinel)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(fs::read(&sentinel).unwrap(), b"foreign\x00\xff\nuntouched");
}
