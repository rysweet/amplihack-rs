# investigation-report: the report file is really written

`amplifier-bundle/recipes/investigation-report.yaml` (#1552). The
`create-investigation-report` step writes the runner-exported `synthesis`
verbatim to `output_dir/investigation_report.md` and exits non-zero if the file
is missing, empty, or whitespace-only. `final-output` runs after it, so the
reported `outputs.report` path exists whenever the recipe succeeds.

- `output_dir` must be non-empty and must not contain `..`.
- Context values are read from the recipe runner's environment exports
  (`UPPER_CASE` with `RECIPE_VAR_<name>` fallback); they are never inserted
  into shell source. This matters for both paths and generated synthesis.
- The synthesis is untrusted text: it is written with `printf` to a temp file
  in `output_dir`, validated before publication, then atomically `mv`'d into
  place. Empty synthesis leaves an existing report untouched. Symlink and
  nonregular report targets are refused without changing them.
- JSON status/output is generated with `jq` so quotes and control characters
  in paths and other context values are encoded safely.

Also removed: the unused `output_dir` context key in `consensus-workflow.yaml`
(nothing in that recipe or its `consensus-*` sub-recipes read it).

Guard: `cargo test -p amplihack-cli --test cli_integration issue_1552_investigation_report_written::`.
