use super::*;

#[test]
fn preserves_existing_short_quarantine_names() {
    assert_eq!(
        quarantine_path_for(
            Path::new("quarantine"),
            Path::new("/home/a/bin/amplihack"),
            0
        ),
        Path::new("quarantine/000-home__a__bin__amplihack")
    );
}

#[test]
fn long_paths_keep_distinct_bounded_names_and_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("quarantine");
    std::fs::create_dir(&run).unwrap();
    let shared = dir.path().join("a".repeat(160)).join("b".repeat(160));
    std::fs::create_dir_all(&shared).unwrap();
    let mut names = Vec::new();
    for suffix in ["first", "second"] {
        let original = shared.join(suffix);
        std::fs::write(&original, suffix).unwrap();
        let target = quarantine_path_for(&run, &original, 0);
        assert!(target.file_name().unwrap().as_encoded_bytes().len() <= 240);
        quarantine_file(&original, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), suffix);
        assert!(!original.exists());
        names.push(target);
    }
    assert_ne!(names[0], names[1]);
}
