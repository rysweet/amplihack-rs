//! Single test binary for the `amplihack` package's own `tests/` directory.
//!
//! Each file here used to be its own test target, so Cargo compiled and linked
//! one executable per file next to the `integration` binary (none of which
//! sccache can cache: they are bin crate types). They are now modules of this
//! one crate root, wired as the single `[[test]] name = "amplihack_bin"`
//! target (`autotests = false` in the manifest). Each module keeps the name
//! its old target had, so a former `cargo test -p amplihack --test <name>` is
//! now `cargo test -p amplihack --test amplihack_bin <name>::`.
//!
//! To add a new test file: create `<name>.rs` here and add a
//! `#[path = "<name>.rs"] mod <name>;` line below. Do NOT add a new
//! `[[test]]` entry to the manifest; the guard
//! `every_amplihack_bin_test_file_is_wired_into_a_test_binary` (in
//! `tests/integration/test_harness_binary_path_contract_test.rs`) fails on any
//! file left out.

#[path = "../../../tests/support/isolated_amplihack.rs"]
mod isolated_amplihack;

#[path = "issue_1445_host_only_litellm.rs"]
mod issue_1445_host_only_litellm;
#[path = "issue_1481_recipe_run_agent_binary.rs"]
mod issue_1481_recipe_run_agent_binary;
#[path = "issue_1525_agent_binary_is_not_subprocess_safe.rs"]
mod issue_1525_agent_binary_is_not_subprocess_safe;
#[path = "issue_1525_migrate_detect_cli_uses_the_resolver.rs"]
mod issue_1525_migrate_detect_cli_uses_the_resolver;
#[path = "issue_537_git_optional_recipes.rs"]
mod issue_537_git_optional_recipes;
#[path = "issue_538_install_completeness.rs"]
mod issue_538_install_completeness;
#[path = "issue_625_update_prompt_subprocess_safe.rs"]
mod issue_625_update_prompt_subprocess_safe;
#[path = "spec_conformance_process.rs"]
mod spec_conformance_process;
