# investigation-report: the report file is really written

`amplifier-bundle/recipes/investigation-report.yaml` (#1552). The
`create-investigation-report` step writes `{{synthesis}}` verbatim to
`{{output_dir}}/investigation_report.md` and exits non-zero if the file is
missing, empty, or whitespace-only. `final-output` runs after it, so the
reported `outputs.report` path exists whenever the recipe succeeds.

- `output_dir` must be non-empty and must not contain `..`.
- The synthesis is untrusted text: it is written through a quoted heredoc
  (never interpolated into `echo`), to a temp file in `output_dir`, then
  `mv`'d into place. A symlinked target is refused.
- `update-discoveries` and `update-patterns` are unchanged.

Also removed: the unused `output_dir` context key in `consensus-workflow.yaml`
(nothing in that recipe or its `consensus-*` sub-recipes read it).

Guard: `cargo test -p amplihack-cli --test issue_1552_investigation_report_written`.
