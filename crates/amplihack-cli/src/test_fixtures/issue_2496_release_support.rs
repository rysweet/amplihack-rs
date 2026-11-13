//! One native fixture build for the library's release publication tests.
use std::{fs, path::Path, process::Command, sync::OnceLock};

use std::{cell::RefCell, rc::Rc};

type Download = Box<dyn FnMut(&str) -> anyhow::Result<Vec<u8>>>;
thread_local! {
    static DOWNLOAD: RefCell<Option<Download>> = RefCell::new(None);
}

/// A scoped transport fixture, never an environment variable or production
/// trust override. Unexpected requests fail immediately, without the network.
pub(crate) struct Downloads {
    previous: Option<Download>,
    pub(crate) calls: Rc<RefCell<Vec<String>>>,
}

impl Downloads {
    pub(crate) fn release(version: &str, native: &Path, include_hooks: bool) -> Self {
        use std::io::Write;
        let platform = match (std::env::consts::ARCH, std::env::consts::OS) {
            ("aarch64", "macos") => "aarch64-apple-darwin",
            ("x86_64", "macos") => "x86_64-apple-darwin",
            ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
            ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
            other => panic!("unsupported release fixture target: {other:?}"),
        };
        let name = format!("amplihack-{platform}.tar.gz");
        let url =
            format!("https://github.com/rysweet/amplihack-rs/releases/download/v{version}/{name}");
        let mut tar = tar::Builder::new(Vec::new());
        for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
            if name == "amplihack-hooks" && !include_hooks {
                continue;
            }
            let bytes = fs::read(native).unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        let metadata = serde_json::to_vec(&serde_json::json!({
            "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
            "assets": [{"name": name, "browser_download_url": url}],
        }))
        .unwrap();
        Self::serving(vec![
            (
                format!(
                    "https://api.github.com/repos/rysweet/amplihack-rs/releases/tags/v{version}"
                ),
                metadata.clone(),
            ),
            (
                "https://api.github.com/repos/rysweet/amplihack-rs/releases/latest".to_owned(),
                metadata,
            ),
            (url, gz.finish().unwrap()),
        ])
    }

    pub(crate) fn serving(responses: Vec<(String, Vec<u8>)>) -> Self {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let recorded = calls.clone();
        let previous = DOWNLOAD.replace(Some(Box::new(move |url| {
            recorded.borrow_mut().push(url.to_owned());
            responses
                .iter()
                .find(|(expected, _)| expected == url)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no release executable available from isolated download source: {url}"
                    )
                })
        })));
        Self { previous, calls }
    }

    pub(crate) fn unavailable() -> Self {
        Self::serving(Vec::new())
    }
}

impl Drop for Downloads {
    fn drop(&mut self) {
        DOWNLOAD.replace(self.previous.take());
    }
}

pub(crate) fn intercept_download(url: &str) -> Option<anyhow::Result<Vec<u8>>> {
    DOWNLOAD.with(|download| download.borrow_mut().as_mut().map(|fetch| fetch(url)))
}

/// Supply real native executable bytes for tests focused on asset staging.
/// The caller holds the environment lock; publication regressions select
/// their own transport explicitly and never use this convenience fixture.
pub(crate) fn with_release<T>(version: &str, action: impl FnOnce() -> T) -> T {
    let native = native_release().join("release");
    let _downloads = Downloads::release(version, &native, true);
    let _env = crate::test_support::EnvGuard::capture(&["RELEASE_FIXTURE_VERSION"]);
    unsafe { std::env::set_var("RELEASE_FIXTURE_VERSION", version) };
    action()
}

pub(crate) fn native_release() -> &'static Path {
    static BUILD: OnceLock<tempfile::TempDir> = OnceLock::new();
    BUILD
        .get_or_init(|| {
            let temp = crate::test_support::tempdir_outside_framework_roots();
            let source = temp.path().join("release.rs");
            fs::write(&source, include_str!("issue_2496_release.rs")).unwrap();
            // Other tests may isolate PATH while this OnceLock initialises.
            // Use the build toolchain, independently of process PATH or HOME.
            let compiler = Path::new(env!("CARGO")).with_file_name(if cfg!(windows) {
                "rustc.exe"
            } else {
                "rustc"
            });
            let output = Command::new(compiler)
                .env("RELEASE_FIXTURE_DEFAULT_VERSION", crate::VERSION)
                .arg("--edition=2024")
                .arg(&source)
                .arg("-o")
                .arg(temp.path().join("release"))
                .output()
                .expect("compile the native release fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            temp
        })
        .path()
}
