//! Native release fixture: version checks and the post-update hand-off.
use std::{env, fs, path::PathBuf, process};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let version = env::var("RELEASE_FIXTURE_VERSION").unwrap_or_else(|_| {
        option_env!("RELEASE_FIXTURE_DEFAULT_VERSION")
            .expect("fixture version is required")
            .to_owned()
    });
    if args == ["--version"] {
        let executable = env::current_exe().unwrap();
        let identity = executable.file_stem().unwrap().to_str().unwrap();
        if env::var("RELEASE_FIXTURE_BAD_BINARY").as_deref() == Ok(identity) {
            match env::var("RELEASE_FIXTURE_BAD_PROBE").as_deref() {
                Ok("exit") => process::exit(23),
                Ok("malformed") => { println!("unparseable"); return; }
                Ok("identity") => { println!("other-tool {version}"); return; }
                Ok("version") => { println!("{identity} 0.0.0"); return; }
                Ok("staged") if executable.parent().unwrap().file_name().unwrap() == "bin" => {
                    process::exit(24);
                }
                _ => {}
            }
        }
        println!("{identity} {version}");
        return;
    }
    assert_eq!(args, ["install", "--force-refresh"]);
    assert_eq!(
        env::var("AMPLIHACK_POST_UPDATE_INSTALL").as_deref(),
        Ok("1")
    );
    let home = PathBuf::from(env::var_os("HOME").unwrap());
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.join(".amplihack/.install.lock"))
        .unwrap();
    // A single non-blocking attempt proves that the parent released its lock.
    lock.try_lock()
        .expect("publication lock must be released before the install child");
    fs::write(
        env::var_os("RELEASE_FIXTURE_RECEIPT").unwrap(),
        env::current_exe()
            .unwrap()
            .canonicalize()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    if env::var_os("RELEASE_FIXTURE_FAIL_INSTALL").is_some() {
        eprintln!("fixture asset installation failed after publication");
        process::exit(42);
    }
}
