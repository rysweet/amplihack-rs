mod cache;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // The wrapper bootstraps into its own private temporary directory. Remove
    // the mapped executable now; no persistent helper binary can be poisoned.
    if args.first().is_some_and(|s| s == "--bootstrap-dir") && args.len() >= 2 {
        let dir = std::fs::canonicalize(args.remove(1)).unwrap_or_default();
        args.remove(0);
        if std::env::current_exe().ok().as_deref() == Some(dir.join("helper").as_path())
            && dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("amplihack-lbug-helper."))
        {
            let _ = std::fs::remove_file(dir.join("helper"));
            let _ = std::fs::remove_dir(&dir);
        }
    }
    std::process::exit(cache::run(args));
}
