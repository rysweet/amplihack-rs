//! Refuse to link a prebuilt lbug of another version.
//!
//! `scripts/lbug-prebuilt.sh` builds lbug's C++ library once per host, and a
//! host can point every build at it with `LBUG_LIBRARY_DIR` and
//! `LBUG_INCLUDE_DIR` (docs/howto/build-lbug-once.md). Those variables name one
//! lbug version. A checkout whose Cargo.lock has moved to another version
//! would compile the crate's bridge against the wrong headers and link the
//! wrong library, so this stops the build and says how to fix it.
//!
//! Only a cache made by the script is checked (it holds a READY file). Any
//! other prebuilt library, or none, is left to the lbug crate.
use std::path::Path;

fn lbug_version_in_lock(lock: &str) -> Option<String> {
    let mut in_lbug = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_lbug = false;
        } else if line == "name = \"lbug\"" {
            in_lbug = true;
        } else if in_lbug && line.starts_with("version = \"") {
            let v = &line["version = \"".len()..];
            return Some(v.trim_end_matches('"').to_string());
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LBUG_LIBRARY_DIR");
    let Ok(lib_dir) = std::env::var("LBUG_LIBRARY_DIR") else {
        return;
    };
    let ready = Path::new(&lib_dir).join("..").join("READY");
    println!("cargo:rerun-if-changed={}", ready.display());
    let Ok(stamp) = std::fs::read_to_string(&ready) else {
        return;
    };
    let built = stamp
        .lines()
        .find_map(|l| l.strip_prefix("lbug "))
        .unwrap_or("")
        .trim()
        .to_string();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let lock_path = Path::new(&manifest).join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let Some(wanted) = std::fs::read_to_string(&lock_path)
        .ok()
        .and_then(|lock| lbug_version_in_lock(&lock))
    else {
        return;
    };
    if built != wanted {
        panic!(
            "LBUG_LIBRARY_DIR={lib_dir} is a prebuilt lbug {built}, but Cargo.lock uses lbug {wanted}. \
             Run scripts/lbug-prebuilt.sh env and set LBUG_LIBRARY_DIR and LBUG_INCLUDE_DIR to what it \
             prints (see docs/howto/build-lbug-once.md), or unset both to build lbug the normal way."
        );
    }
}
