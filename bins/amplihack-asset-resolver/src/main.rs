use amplihack_cli::resolve_bundle_asset;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: amplihack-asset-resolver <asset>");
        eprintln!("  <asset> is either:");
        eprintln!("    - a named asset: helper-path | session-tree-path | multitask-orchestrator");
        eprintln!("    - a relative path starting with 'amplifier-bundle/'");
        std::process::exit(2);
    }

    std::process::exit(resolve_bundle_asset::run_cli(&args[1]));
}
