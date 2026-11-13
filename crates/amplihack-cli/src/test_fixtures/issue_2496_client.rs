//! Disposable native client and installer tripwire, compiled by the process tests.
use std::{env, fs, path::Path, process};

fn main() {
    let argv: Vec<_> = env::args().collect();
    let tool = Path::new(&argv[0]).file_name().unwrap().to_str().unwrap();
    if argv.get(1).map(String::as_str) == Some("--version") {
        println!(
            "{}",
            match tool {
                "node" => "v24.0.0",
                "npm" => "10.9.0",
                "uv" => "uv 0.8.0",
                "codex" => "codex-cli 0.160.0",
                "claude" => "2.1.100 (Claude Code)",
                _ => "1.0.0",
            }
        );
        return;
    }
    if tool == "npm" && argv.get(1).map(String::as_str) == Some("show") {
        let cache = Path::new(&env::var_os("HOME").unwrap()).join(".npm");
        fs::create_dir_all(cache.join("_logs")).unwrap();
        fs::write(cache.join("_logs/query.log"), b"npm registry query").unwrap();
        fs::write(cache.join("_update-notifier-last-checked"), b"queried").unwrap();
        fs::write(env::var_os("ACQUISITION_QUERY").unwrap(), argv.join(" ")).unwrap();
        println!("999.0.0");
        return;
    }
    if matches!(tool, "npm" | "uv" | "cargo" | "curl" | "wget") {
        fs::write(env::var_os("ACQUISITION_TRIPWIRE").unwrap(), argv.join(" ")).unwrap();
        eprintln!("isolated installer tripwire");
        process::exit(91);
    }
    println!("CLIENT_LAUNCHED");
}
