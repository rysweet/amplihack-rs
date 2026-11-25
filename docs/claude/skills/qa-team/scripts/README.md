# Running native Rust QA

For a Rust project, run its existing Cargo integration tests from the project
root. The compatibility alias uses this folder for execution guidance. It does
not need a separate script runner.

1. Find the integration test that exercises the command or workflow being
   changed. Read the package manifest to identify its package and test target.
2. Run the whole affected integration test target with `cargo test --locked`.
   Use Cargo's `-p` and `--test` options when the workspace has several packages
   or integration test targets.
3. Use an isolated temporary home for installation scenarios. Give child
   processes that home explicitly, together with the temporary configuration
   roots used by each supported tool. Keep scratch under the supplied `TMPDIR`.
4. Assert exit status and observable filesystem changes. Save command output
   when a scenario fails so that the failure can be investigated.
5. Run the repository's required checks after the affected scenarios pass.

Read `../tests/README.md` for filesystem scenario design and `../examples/cli/`
for command interaction examples. Follow the project's existing test helpers
for child-process isolation and cleanup.
