use crate::support::{KEY, archive, sidecar};
use lbug_cache::artifact::{ExtractionLimits, extract_archive, verify_checksum};
use tar::EntryType;

#[test]
fn checksum_accepts_exact_sidecar_and_rejects_corruption_or_ambiguous_syntax() {
    let bytes = b"native archive fixture";
    let valid = sidecar(bytes);
    verify_checksum(bytes, &valid, &format!("{KEY}.tar.gz")).unwrap();
    assert!(verify_checksum(b"corrupt", &valid, &format!("{KEY}.tar.gz")).is_err());
    for invalid in [
        String::new(),
        valid.replace(&format!("{KEY}.tar.gz"), "different.tar.gz"),
        format!("{valid}{valid}"),
        valid.replace("  ", "  ../"),
        format!("{valid}unparsed suffix"),
        format!("{}  {KEY}.tar.gz\n", "g".repeat(64)),
    ] {
        assert!(verify_checksum(bytes, &invalid, &format!("{KEY}.tar.gz")).is_err());
    }
}

#[test]
fn extractor_accepts_regular_payload_without_preserving_privileged_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = archive(&[("include/lbug.hpp", EntryType::Regular, b"// test header\n")]);
    extract_archive(&bytes[..], dir.path(), &ExtractionLimits::default()).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("include/lbug.hpp")).unwrap(),
        b"// test header\n"
    );
}

#[test]
fn extraction_rejects_traversal_links_special_members_and_duplicate_names() {
    for (name, kind) in [
        ("../escaped", EntryType::Regular),
        ("include/../../escaped", EntryType::Regular),
        ("include/link", EntryType::Symlink),
        ("include/hard", EntryType::Link),
        ("include/fifo", EntryType::Fifo),
        ("include/device", EntryType::Char),
        ("include/sparse", EntryType::GNUSparse),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        let bytes = archive(&[(name, kind, b"")]);
        assert!(
            extract_archive(&bytes[..], &staging, &ExtractionLimits::default()).is_err(),
            "{name}"
        );
        assert!(!dir.path().join("escaped").exists());
    }
    for second in ["include/lbug.hpp", "include/LBUG.hpp"] {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            ("include/lbug.hpp", EntryType::Regular, b"first"),
            (second, EntryType::Regular, b"replacement"),
        ]);
        assert!(extract_archive(&bytes[..], dir.path(), &ExtractionLimits::default()).is_err());
    }
    // Even a broken extractor can touch only this test's owned absolute path.
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let staging = dir.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let escaped = dir.path().join("absolute");
    let bytes = archive(&[(escaped.to_str().unwrap(), EntryType::Regular, b"poison")]);
    assert!(extract_archive(&bytes[..], &staging, &ExtractionLimits::default()).is_err());
    assert!(!escaped.exists());
}

#[test]
fn extraction_limits_apply_to_actual_expansion_and_member_counts() {
    // Small injected limits exercise the same boundaries without allocating GiB.
    let limits = ExtractionLimits {
        compressed_bytes: 4096,
        expanded_bytes: 8,
        file_bytes: 4,
        members: 2,
        path_bytes: 40,
        path_components: 3,
        manifest_bytes: 4,
    };
    for entries in [
        vec![("large", EntryType::Regular, b"12345".as_slice())],
        vec![
            ("a", EntryType::Regular, b"".as_slice()),
            ("b", EntryType::Regular, b"".as_slice()),
            ("c", EntryType::Regular, b"".as_slice()),
        ],
        vec![("a/b/c/d", EntryType::Regular, b"".as_slice())],
        vec![("manifest.json", EntryType::Regular, b"12345".as_slice())],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&entries);
        assert!(extract_archive(&bytes[..], dir.path(), &limits).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let bytes = archive(&[("exact", EntryType::Regular, b"1234")]);
    extract_archive(&bytes[..], dir.path(), &limits).unwrap();
    let mut expansion = limits;
    expansion.members = 3;
    let bytes = archive(&[
        ("a", EntryType::Regular, b"123"),
        ("b", EntryType::Regular, b"456"),
        ("c", EntryType::Regular, b"789"),
    ]);
    let dir = tempfile::tempdir().unwrap();
    assert!(extract_archive(&bytes[..], dir.path(), &expansion).is_err());
    expansion.compressed_bytes = (bytes.len() - 1).try_into().unwrap();
    expansion.expanded_bytes = 100;
    let dir = tempfile::tempdir().unwrap();
    assert!(extract_archive(&bytes[..], dir.path(), &expansion).is_err());
}
