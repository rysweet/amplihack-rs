//! Standalone Rust fixture, compiled explicitly by the issue #2441 integration harness.
//! NUL-separated fields preserve argument boundaries without shell serialisation.
use std::io::Read;

fn main() {
    let mut args = std::env::args().skip(1);
    let tag = args.next().expect("record name");
    assert!(!tag.contains('/') && !tag.contains(".."));
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin).unwrap();
    let mut fields = vec![
        std::env::current_dir()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
        stdin,
    ];
    fields.extend(args);
    let mut record = fields.join("\0");
    record.push('\0');
    let dir = std::env::var_os("QA_RECORD_DIR").expect("isolated record directory");
    let dir = std::path::Path::new(&dir);
    if let Some(root) = std::env::var_os("QA_COMPLETION_ROOT") {
        use std::os::unix::fs::PermissionsExt;
        let directories: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("autodrive-qa-result.")
            })
            .collect();
        assert_eq!(
            directories.len(),
            1,
            "one private directory for this invocation"
        );
        let private = &directories[0];
        assert_eq!(
            std::fs::symlink_metadata(private)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        std::fs::write(
            dir.join(format!("{tag}.completion")),
            private.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
    }
    std::fs::write(dir.join(tag), record).unwrap();
    println!("QA-RECORDER-STDOUT");
    eprintln!("QA-RECORDER-STDERR");
}
