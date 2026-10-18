//! Name a test the way libtest lists it inside `cli_integration`.

/// The libtest name of test `name` in the module at `module_path` (pass
/// `module_path!()`). As a module of the `cli_integration` binary a test is
/// listed as `<module>::<name>`, and `--exact` needs that full path;
/// `module_path!()` is `<crate>::<module>`.
pub fn test_path(module_path: &str, name: &str) -> String {
    match module_path.split_once("::") {
        Some((_, module)) => format!("{module}::{name}"),
        None => name.to_owned(),
    }
}
