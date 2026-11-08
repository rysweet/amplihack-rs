# Workflow checkpoint legacy characterisation fixtures (#1554)

These bytes pin the behaviour of the Python helpers at 918c256cf, which the
Rust `amplihack internal workflow-checkpoint` engine replaces.

- `legacy_parser/no-final-lf.advert`, `crlf.advert` and `mixed-oid-widths.advert`
  are copied byte for byte from cc's legacy-parser-diagnostics packet
  (pr2020-draft-review-20261107T211536Z). The 918c `ORIGIN_REF_PY` fragment
  (`amplifier-bundle/tools/workflow_pr_head_sync.sh:83-92`, SHA-256
  e4fbb280abb1693c1c9e263bd70b076cca4220e2219368f72fd993c28f0b6472) accepts
  each with exit 0: targets `refs/heads/task`, `refs/heads/task` and
  `refs/heads/a`, transport status 0.
- `SHA256SUMS` is checked by a test. `.gitattributes` marks every file `-text`
  so checkout never rewrites a CR or adds a final LF.

The remaining expected statuses in the tests were recorded on 2026-11-07 by
running the fragments extracted with `git show 918c256cf:<path>` under
`python3 -I`, outside the repository. Regenerate them only from the 918c
reference, never from the Rust output.
