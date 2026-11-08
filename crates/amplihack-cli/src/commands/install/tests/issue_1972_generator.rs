//! Issue #1972 D5, D6: `scripts/generate-bundle-manifest.sh` lists exactly the
//! files `of_source` hashes, in the format `parse` reads.

use super::super::bundle_manifest::{of_source, parse};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn script() -> PathBuf {
    repo_root().join("scripts/generate-bundle-manifest.sh")
}

/// Run the generator with its own `TMPDIR`, returned so a test can check that
/// nothing is left behind.
fn generate_in(dir: &OsStr, tmp: &Path) -> Output {
    Command::new("bash")
        .arg(script())
        .arg(dir)
        .env("TMPDIR", tmp)
        .output()
        .expect("run the generator")
}

fn generate(dir: &Path) -> Output {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let output = generate_in(dir.as_os_str(), tmp.path());
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        0,
        "the generator must remove its temporary files"
    );
    output
}

fn write(root: &Path, rel: impl AsRef<Path>, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Every exclusion `walk_tree` applies, the type-dependent cases, and names
/// whose byte order differs from a naive order.
fn parity_fixture(root: &Path) {
    write(root, "CLAUDE.md", "root\n");
    write(root, "B.md", "upper\n");
    write(root, "a.b", "dot\n");
    write(root, "a/b", "slash\n");
    write(root, "a-b", "dash\n");
    write(root, ".hidden", "dotfile\n");
    write(root, "agents/é.md", "non-ascii\n");
    write(
        root,
        "tools/__pycache__/x.cpython-312.pyc",
        "excluded dir\n",
    );
    write(root, "tools/.pytest_cache/v/cache", "excluded dir\n");
    write(root, "tools/node_modules/dep/index.js", "excluded dir\n");
    write(root, "tools/stale.pyc", "excluded file\n");
    write(root, "tools/stale.pyo", "excluded file\n");
    // A file called node_modules is kept; a directory called x.pyc is walked.
    write(root, "tools/node_modules-file/node_modules", "kept\n");
    write(root, "tools/x.pyc/inside.txt", "kept\n");
    write(root, "empty.txt", "");
    std::os::unix::fs::symlink("CLAUDE.md", root.join("link-to-file")).unwrap();
    std::os::unix::fs::symlink("agents", root.join("link-to-dir")).unwrap();
    let status = Command::new("mkfifo")
        .arg(root.join("tools/a-fifo"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn issue_1972_generator_matches_of_source_on_a_fixture() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("amplifier-bundle");
    parity_fixture(&root);

    let output = generate(&root);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let expected = of_source(&root).unwrap();
    assert_eq!(parse(&text).unwrap(), expected);
    assert_eq!(text, expected.to_text(), "byte-identical canonical text");
    assert!(text.contains("  tools/node_modules-file/node_modules\n"));
    assert!(text.contains("  tools/x.pyc/inside.txt\n"));
    assert!(!text.contains("__pycache__") && !text.contains("a-fifo") && !text.contains("link-to"));

    let again = generate(&root);
    assert_eq!(
        String::from_utf8(again.stdout).unwrap(),
        text,
        "deterministic"
    );
}

fn assert_rejected(rel: &OsStr) {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("amplifier-bundle");
    write(&root, "ok.md", "ok\n");
    let path = root.join("agents").join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "x").unwrap();

    let output = generate(&root);

    assert_eq!(output.status.code(), Some(1), "{rel:?}");
    assert!(output.stdout.is_empty(), "nothing on stdout for {rel:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("generate-bundle-manifest:"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn issue_1972_generator_rejects_paths_the_format_cannot_carry() {
    for rel in ["new\nline", "ctrl\u{1}x", "back\\slash", "repl\u{fffd}x"] {
        assert_rejected(OsStr::new(rel));
    }
    // Only a leading `*` is the binary-mode marker; test it at the root.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("amplifier-bundle");
    write(&root, "*star", "x");
    assert_eq!(generate(&root).status.code(), Some(1));
}

#[cfg(target_os = "linux")]
#[test]
fn issue_1972_generator_rejects_a_name_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    assert_rejected(OsStr::from_bytes(b"bad\xffname"));
}

#[test]
fn issue_1972_generator_rejects_an_empty_tree_and_misuse() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let empty = temp.path().join("empty");
    fs::create_dir_all(empty.join("only/dirs")).unwrap();
    write(&empty, "__pycache__/x.pyc", "excluded\n");
    let output = generate(&empty);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());

    let missing = generate(&temp.path().join("missing"));
    assert_eq!(missing.status.code(), Some(2));
    std::os::unix::fs::symlink(&empty, temp.path().join("link")).unwrap();
    assert_eq!(generate(&temp.path().join("link")).status.code(), Some(2));

    let no_args = Command::new("bash").arg(script()).output().unwrap();
    assert_eq!(no_args.status.code(), Some(2));
}

#[test]
fn issue_1972_generator_matches_of_source_on_the_repository_bundle() {
    let bundle = repo_root().join("amplifier-bundle");

    let output = generate(&bundle);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(parse(&text).unwrap(), of_source(&bundle).unwrap());

    // The published asset works with the standard tool, run from the bundle.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let manifest = temp.path().join("amplifier-bundle-manifest.txt");
    fs::write(&manifest, &text).unwrap();
    let (tool, args): (&str, &[&str]) = if Command::new("sha256sum")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        ("sha256sum", &["--strict", "--quiet", "-c"])
    } else {
        ("shasum", &["-a", "256", "--strict", "--quiet", "-c"])
    };
    let check = Command::new(tool)
        .args(args)
        .arg(&manifest)
        .current_dir(&bundle)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{tool} -c failed: {}",
        String::from_utf8_lossy(&check.stdout)
    );
}
