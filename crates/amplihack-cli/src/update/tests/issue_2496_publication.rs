//! Update publication uses the downloaded binaries, followed by the new
//! executable. The fixture's one-shot lock attempt detects an unsafe hand-off
//! without a timeout, a polling loop or a blocked install child.
use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    archive: PathBuf,
    old: PathBuf,
    release: UpdateRelease,
}

impl Fixture {
    fn new(native: &Path) -> Self {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path().join("home");
        let old = home.join(".amplihack/releases/old");
        let archive = temp.path().join("extracted-archive");
        fs::create_dir_all(old.join("bin")).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir(&archive).unwrap();
        fs::write(old.join(".complete"), b"").unwrap();
        fs::write(home.join(".amplihack/.installed-version"), b"0.0.0\n").unwrap();
        for name in ["amplihack", "amplihack-hooks"] {
            fs::write(old.join("bin").join(name), b"old executable bytes\n").unwrap();
            symlink(
                old.join("bin").join(name),
                home.join(".local/bin").join(name),
            )
            .unwrap();
            fs::copy(native, archive.join(name)).unwrap();
        }
        let version = semver::Version::parse(crate::VERSION).unwrap();
        Self {
            _temp: temp,
            home,
            archive,
            old,
            release: UpdateRelease {
                version: format!("{}.0.0", version.major + 1),
                asset_url: String::new(),
                checksum_url: None,
            },
        }
    }

    fn publish(&self) -> Result<()> {
        install_into_release(
            &self.home,
            &self.release,
            &self.archive.join("amplihack"),
            &self.archive.join("amplihack-hooks"),
            &self.home.join(".local/bin/amplihack"),
            &self.home.join(".local/bin/amplihack-hooks"),
        )
    }
}

#[test]
fn explicit_update_switches_to_downloaded_bytes_and_invokes_the_new_executable() {
    const CHILD: &str = "AMPLIHACK_2496_UPDATE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let native = home.parent().unwrap().join("extracted-archive/amplihack");
        let version = std::env::var("RELEASE_FIXTURE_VERSION").unwrap();
        let downloads =
            crate::test_support::release_fixture::Downloads::release(&version, &native, true);
        let result = crate::update::run_update(false);
        if std::env::var_os("RELEASE_FIXTURE_FAIL_INSTALL").is_some() {
            let error = result.expect_err("asset failure must reach the explicit update caller");
            assert!(format!("{error:#}").contains("post-update install"));
        } else {
            result.expect("explicit update must publish and install using the new executable");
        }
        assert!(!crate::install_lock::held_by_current_thread());
        assert!(
            downloads
                .calls
                .borrow()
                .iter()
                .any(|url| url.ends_with("/releases/latest"))
        );
        assert!(
            downloads
                .calls
                .borrow()
                .iter()
                .any(|url| url.ends_with(".tar.gz"))
        );
        return;
    }
    let native = crate::test_support::release_fixture::native_release().join("release");
    for fail_assets in [false, true] {
        let fixture = Fixture::new(&native);
        let receipt = fixture.home.join("install-child-receipt");
        // Run the real updater from an eligible private release. Its
        // current-executable target selection cannot reach the checkout.
        let harness = fixture.old.join("bin/update-test-child");
        fs::hard_link(std::env::current_exe().unwrap(), &harness).unwrap();
        let mut child = Command::new(&harness);
        child.env_clear().args([
                "--exact",
                "update::install::issue_2496_publication::explicit_update_switches_to_downloaded_bytes_and_invokes_the_new_executable",
                "--test-threads=1", "--nocapture",
            ]).env(CHILD, "1")
                .env("HOME", &fixture.home)
                .env("TMPDIR", fixture._temp.path())
                .env("PATH", fixture.home.join(".local/bin"))
                .env("CLAUDE_CONFIG_DIR", fixture.home.join(".claude"))
                .env("CODEX_HOME", fixture.home.join(".codex"))
                .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", fixture.home.join("profiles"))
                .env("XDG_CONFIG_HOME", fixture.home.join(".config"))
                .env("XDG_CACHE_HOME", fixture.home.join(".cache"))
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .env("RELEASE_FIXTURE_VERSION", &fixture.release.version)
                .env("RELEASE_FIXTURE_RECEIPT", &receipt);
        if fail_assets {
            child.env("RELEASE_FIXTURE_FAIL_INSTALL", "1");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the updater child must run exactly one test"
        );
        let installed = fixture.home.join(".local/bin/amplihack");
        let active = fs::read_link(&installed).unwrap();
        let root = active.parent().unwrap().parent().unwrap();
        assert!(root.join(".complete").is_file());
        assert_eq!(
            crate::commands::install::release_layout::read_installed_by(root)
                .unwrap()
                .version,
            fixture.release.version
        );
        for name in ["amplihack", "amplihack-hooks"] {
            let link = fixture.home.join(".local/bin").join(name);
            assert_eq!(fs::read_link(&link).unwrap(), root.join("bin").join(name));
            assert!(
                fs::read(&link).unwrap() == fs::read(fixture.archive.join(name)).unwrap(),
                "{name} must match the downloaded bytes"
            );
            assert_eq!(
                fs::read(fixture.old.join("bin").join(name)).unwrap(),
                b"old executable bytes\n"
            );
        }

        assert_eq!(
            fs::read(&receipt).unwrap(),
            active
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
        );

        assert!(
            fs::read(&installed).unwrap() == fs::read(&native).unwrap(),
            "asset failure must preserve the published release bytes"
        );
    }
}

#[test]
fn explicit_update_refuses_non_executable_companions_before_allocating_a_release() {
    let native = crate::test_support::release_fixture::native_release().join("release");
    let fixture = Fixture::new(&native);
    fs::set_permissions(
        fixture.archive.join("amplihack-hooks"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let links: Vec<_> = ["amplihack", "amplihack-hooks"]
        .iter()
        .map(|name| fs::read_link(fixture.home.join(".local/bin").join(name)).unwrap())
        .collect();
    let releases = fixture.home.join(".amplihack/releases");

    let result = crate::test_support::with_env(
        &[
            ("HOME", Some(fixture.home.as_os_str())),
            ("AMPLIHACK_HOME", None),
            ("AMPLIHACK_SKIP_AUTO_INSTALL", Some("1".as_ref())),
            (
                "RELEASE_FIXTURE_VERSION",
                Some(fixture.release.version.as_ref()),
            ),
        ],
        || fixture.publish(),
    );

    assert_eq!(
        fs::read_dir(&releases).unwrap().count(),
        1,
        "invalid binaries must fail before persistent staging"
    );
    assert!(
        result.is_err(),
        "an unreadable or non-executable archive companion is not publishable"
    );
    for (name, target) in ["amplihack", "amplihack-hooks"].iter().zip(links) {
        assert_eq!(
            fs::read_link(fixture.home.join(".local/bin").join(name)).unwrap(),
            target
        );
    }
}
