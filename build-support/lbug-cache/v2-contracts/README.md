# Deferred v2 contracts

These RED Rust contracts belong to the earlier cross-host publication proposal.
They are retained for a follow-up; they are not v1 regression tests and their
proposed helper API is not implemented. The spec owner's decision in
[comment6616249815](https://github.com/mistt-repo/Product-Management/issues/328#issuecomment-6616249815)
explicitly excludes vendoring, publication, and a patched lbug build script.

Run the implemented v1 contracts with:

```sh
python3 -m unittest discover -s tests -p 'test_issue_1917_lbug_*.py' -v
npm test
```

Native acceptance remains separate from the small Cargo fingerprint fixture.
