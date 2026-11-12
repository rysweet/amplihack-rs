//! Standalone Rust fixture, compiled by the issue #2441 integration harness.
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
    std::fs::write(std::path::Path::new(&dir).join(tag), record).unwrap();
    println!("QA-RECORDER-STDOUT");
    eprintln!("QA-RECORDER-STDERR");
}
