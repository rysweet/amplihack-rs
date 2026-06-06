use amplihack_cli::resolve_bundle_asset;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let mut args = std::env::args();
    let _program = args.next();

    match (args.next(), args.next()) {
        (Some(asset), None) => std::process::exit(resolve_bundle_asset::run_cli(&asset)),
        _ => {
            eprintln!(
                "{}",
                resolve_bundle_asset::usage_text("amplihack-asset-resolver")
            );
            std::process::exit(2);
        }
    }
}
