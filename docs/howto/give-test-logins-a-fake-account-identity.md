---
title: Give Test Logins a Fake Account Identity
type: howto
updated: 2026-11-07
related: issue #1753, issue #1693, issue #1699, issue #1946
---

# Give Test Logins a Fake Account Identity

Since issue #1753, rotation and manual switching decide which account a
login belongs to from the login itself: the credential in a profile's
`.credentials.json` is presented to the identity endpoint, and the account
the server returns is the account of that login. A session can rewrite
`~/.claude.json` at any time, so its `oauthAccount` block is used only for
display and is never enough to authorise a swap, a backfill or a move. See
[Rotation login identity](../reference/profile-command.md#rotation-login-identity).

A test that runs `profile rotate`, `profile switch`, `profile usage` or
`profile auto-switch` therefore needs fake logins whose credential the test
can resolve to an account, and a local endpoint that answers for them. This
guide shows how to provide both, and how to change the account a login
names partway through a test.

## Contents

- [Choose a helper](#choose-a-helper)
- [Use the rotate fixture](#use-the-rotate-fixture)
- [Use `give_logins` in other fixtures](#use-give_logins-in-other-fixtures)
- [Seed a login in a rotate unit test](#seed-a-login-in-a-rotate-unit-test)
- [Change the account a login names](#change-the-account-a-login-names)
- [Test a profile with no recorded account](#test-a-profile-with-no-recorded-account)
- [Test that a command creates no state](#test-that-a-command-creates-no-state)
- [Rules for these tests](#rules-for-these-tests)
- [Check your test](#check-your-test)

## Choose a helper

| Your test uses | Helper | File |
|---|---|---|
| `Fixture::new()` from the rotate support (profiles `work`, `spare`, `extra`) | Built in. `login(path)` writes each credential and the fixture starts its own endpoint. | `crates/amplihack-cli/tests/support/profile_rotate.rs` |
| Its own fixture with named profiles under a temp `HOME` | `give_logins(home, root, live, stored)` | `crates/amplihack-cli/tests/support/profile_default.rs` |
| A unit test inside `commands/profile/rotate` | `tests_fixture_identity::body(path)` to seed, `tests_fixture_identity::read(dir)` to identify | `crates/amplihack-cli/src/commands/profile/rotate/tests_fixture_identity.rs` |

The two integration helpers bind their endpoint to `127.0.0.1` on a free port, accept only
tokens that start with their own fake prefix, and stop the server when the
owner is dropped. Neither reads the real `HOME`. The integration
`login(path)` and `give_logins` assert that every path they write is under the system temp
directory. `private_file(..)` does not check its path, so build the path
from the fixture's `home` or `root`.

## Use the rotate fixture

`Fixture::new()` creates three profiles and makes `work` live in
`~/.claude`. Each credential is written by `login(path)`:

- the access token is `fake-rotation1753-` followed by the hex of the
  `oauthAccount` block in the `.claude.json` next to the credential (for
  `~/.claude/.credentials.json`, that is `$HOME/.claude.json`);
- for `~/.claude/.credentials.json` only, if
  `$HOME/profiles/.auto-switch.d/rotate/journal.json` (under the fixture's
  root) exists and is in phase `s1` to `s4`, the token names the journal's incoming account instead of
  `$HOME/.claude.json`, as a recovery refresh would; a recovery test that
  expects the account from `$HOME/.claude.json` must clear or finish the
  journal first;
- the fixture's endpoint decodes that hex and answers
  `GET /api/oauth/profile` with the same account and organization;
- `GET /api/oauth/usage` answers `503`, so measurement falls back to the
  canned figures set by `f.usage(..)` and never triggers a token refresh.

The fixture also seeds a [recorded account](../reference/profile-command.md#recorded-account)
for each profile in `<root>/.auto-switch.d/expected/`. Commands built with
`f.cmd(..)`, `f.run(..)` or `f.ok(..)` already point at the endpoint, so no
further setup is needed:

```rust
let f = Fixture::new();
let out = f.ok(&["profile", "rotate"]);
assert!(text(&out.stdout).contains("swap work->spare"), "{}", text(&out.stdout));
f.assert_each_login_once();
```

## Use `give_logins` in other fixtures

`give_logins` writes, for the live profile and each stored profile:

- `<root>/<name>/.claude.json` with account `uuid-<name>` and the shared
  fake organization;
- a recorded account in `<root>/.auto-switch.d/expected/<name>.json`;
- a credential with token `fake-default1753-<name>`, in `~/.claude` for the
  live profile and in `<root>/<name>` for stored ones.

It returns a `LoginIdentities` guard. Keep it in the fixture for the whole
test and apply it to every command after `env_clear()`:

```rust
use crate::support::profile_default::{give_logins, LoginIdentities};

struct Fixture {
    home: PathBuf,
    root: PathBuf,
    identity: Option<LoginIdentities>,
}

impl Fixture {
    fn with_logins(mut self) -> Self {
        self.identity = Some(give_logins(&self.home, &self.root, "alpha", &["beta"]));
        self
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = isolated(env!("CARGO_BIN_EXE_amplihack"));
        c.env("HOME", &self.home).args(args);
        self.identity.iter().for_each(|i| i.configure(&mut c));
        c
    }
}
```

`configure` sets `AMPLIHACK_PROFILE_USAGE_API_BASE` and
`AMPLIHACK_PROFILE_USAGE_TOKEN_URL` to the guard's loopback address.

## Seed a login in a rotate unit test

Unit tests in `crates/amplihack-cli/src/commands/profile/rotate` do not
start an endpoint. Under `cfg(test)`, `cred_identity::DEFAULT_READ` is
`tests_fixture_identity::read`, which takes the account from a synthetic
`task1753FakeIdentity` block in the credential file itself. Write each
credential with `tests_fixture_identity::body(path)`. Existing tests
already do this, for example `tests_live_readers.rs`,
`tests_adopt_unreadable.rs` and the `login(path)` helpers in
`tests_swap.rs` and `tests_identify.rs`. Import the module by its full path, which works
from any module under `rotate`:

```rust
use crate::commands::profile::rotate::tests::tests_fixture_identity::{body, read};

let tmp = tempfile::tempdir().unwrap();
let creds = tmp.path().join("home/.claude/.credentials.json");
std::fs::create_dir_all(creds.parent().unwrap()).unwrap();
std::fs::write(&creds, body(&creds)).unwrap();
assert_eq!(read(&tmp.path().join("home/.claude")).unwrap().key(), "u-work");
```

`body(path)` returns the credential JSON: token `SENTINEL`, and the
account named in the session's `.claude.json`. When the credential's
directory ends in `.claude`, the session is that directory's parent, so
`<tmp>/home/.claude` reads `<tmp>/home/.claude.json`; otherwise the session
is the directory itself, so a stored profile `<tmp>/root/<name>` reads
`<tmp>/root/<name>/.claude.json`. When that file names no account, it uses
`u-<name>` with `<name>@example.com`, where `<name>` is `work` for any
directory ending in `.claude` and the directory name otherwise.

`body(path)` also registers the fixture's root, the first directory below
the system temp directory (`<tmp>` above), as a directory the reader may
open. Registration is never removed, even after the `TempDir` is deleted;
tempfile names are random, so reuse is unlikely.

`body(path)` panics when `path`:

- is not under the system temp directory;
- contains a `..` component;
- lies directly in the system temp directory, so that the temp directory
  itself would become the root.

`read(dir)` returns `Err("not a task fixture")`, without opening any file,
unless `dir` is absolute, contains no `..` component and lies at or below a
registered root. The comparison is by whole path component and nothing is
canonicalised, so each of these is refused even when it holds a valid
credential:

| Directory passed to `read` | Why it is refused |
|---|---|
| A `tempfile::tempdir()` that no `body()` call has seeded | Not registered |
| `<tmp>/home/../../<other tempdir>` | Contains `..` |
| `<tmp>xyz`, a sibling whose name starts with the fixture's name | A different component from `<tmp>` |

The remaining errors are unchanged: `"fake login missing"`,
`"fake login invalid"`, `"fake identity missing"` and
`"fake identity invalid"`.

A credential written by any means other than `body()` is identifiable only
if some `body()` call has already registered its fixture root. A swap that
moves a login between `<tmp>/home/.claude` and `<tmp>/root/<name>` stays
readable, because both lie under the same root. If a new fixture needs the
reader for a directory that `body()` has not seeded, seed one credential in
it with `body()`. Do not widen the check in `read()`: before #1946 it
accepted any directory under the system temp directory, including other
tests' fixtures.

## Change the account a login names

When a test rewrites a `.claude.json` so that a login names another
account, the credential must be re-issued as well. Otherwise the server
identity still names the old account, and the test is exercising a
conflict rather than the case it describes.

Call `login(path)` again on the credential beside the rewritten file. It
writes the file in place, so the inode, which the rotate tests use to
follow a login, does not change; assert that it has not. The example below is
shortened from the #1592 test of the same name; the committed test has
further assertions:

```rust
#[test]
fn an_unknown_login_is_adopted_as_default_original() {
    let f = Fixture::new();
    let home_json = serde_json::json!({
        "oauthAccount": account_json("stranger"),
        "primaryApiKey": "SECRET-KEY-HOME"
    });
    private_file(&f.home.join(".claude.json"), &home_json.to_string());
    assert_eq!(login(&f.home_login()), f.live_ino);
    f.usage("stranger", 95, 10);
    let out = f.ok(&["profile", "rotate"]);
    assert!(text(&out.stdout).contains("swap default-original->spare"));
}
```

Do not re-issue the credential when the test is about a session rewriting
`~/.claude.json` behind the rotator's back. In that case the mismatch is the
point. `stale_session_b_does_not_refuse_swapping_credential_a_to_c` in
`tests/issue_1753_login_identity.rs` is that test: `~/.claude.json` names
another profile's account, the credential's server identity matches the
ledger's profile, and the swap goes ahead.

## Test a profile with no recorded account

Both helpers seed a record for every profile. A test whose premise is "no
record" must remove the fixture's records, using a path built from the
fixture root. The #1650 tests remove the whole
`<root>/.auto-switch.d/expected` directory. The example below is shortened
from the #1650 test of the same name, which also checks the `note:` line
from `profile auto-switch status`:

```rust
#[test]
fn without_a_record_the_duplicate_warning_cannot_tell() {
    let f = Fixture::new();
    fs::remove_dir_all(f.root.join(".auto-switch.d/expected")).unwrap();
    set_account(&f, "spare", &account_json("extra"));
    login(&f.slot("spare"));
    let stderr = text(&f.run(&["profile", "usage"]).stderr);
    assert!(stderr.contains(&format!("warning: {CANNOT_TELL}\n")), "{stderr}");
}
```

Without a record, amplihack still measures the profile and reports
"cannot tell" for the duplicate check. It does not backfill a record or
move a login on unread evidence;
`unread_duplicate_evidence_cannot_authorize_backfill_or_movement` in
`tests/issue_1753_login_identity.rs` checks this.

When one of the duplicates is the login in `~/.claude`, the warning names
`'~/.claude'` first and the stored profile second, and without a record it
gives a fix for each side. The test
`a_profile_duplicating_the_home_login_is_warned_with_home_first` in
`tests/issue_1650_profile_identity.rs` sets this up the same way: it
removes the records, rewrites `extra`'s `.claude.json` to name `work`, and
calls `login(&f.slot("extra"))`.

## Test that a command creates no state

`profile usage` writes nothing under `<root>/.auto-switch.d` unless history
recording is on. Because the fixtures create that directory while seeding
records, a test of this rule removes it first. The example is the start of
`history_is_written_only_for_a_whole_unfiltered_run_with_recording_on` in
`tests/issue_1797_capacity_summary.rs`; `state_dir` and `usage_ok` are
helpers in that file:

```rust
let f = Fixture::new();
fs::remove_dir_all(state_dir(&f)).unwrap();
usage_ok(&f, &[], &[]);
assert!(
    !state_dir(&f).exists(),
    "profile usage never creates .auto-switch.d"
);
```

## Rules for these tests

- Run every command through `isolated(..)` with a temp `HOME`. See
  [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md).
- Never read, print, log or copy a credential file. Pass command output
  through `assert_no_secrets(&out)`, which rejects the sentinels, account
  uuids, e-mail addresses and the `fake-rotation1753-`, `fake-default1753-`
  and `fake-task1753-` tokens. Do not add `dbg!` or `println!` near tokens,
  `.credentials.json` or `.claude.json` bodies.
- Bind fake endpoints to `127.0.0.1` only.
- Fix the setup, not the assertion. If a test still fails once its fixture
  matches its premise, the fault is in the code.
- Conflicting or missing evidence must move nothing. In
  `tests/issue_1753_login_identity.rs`,
  `identity_refusal_is_logged_on_each_affected_rotate_cycle` asserts that
  each `profile rotate` cycle adds an `ALERT:` line to the rotate log and
  that no login moved.
  `credential_conflict_moves_nothing_even_when_session_matches_record` and
  `unavailable_trusted_identity_moves_nothing` assert that `profile switch`
  fails with a message and that no login or ledger entry changed;
  `unread_duplicate_evidence_cannot_authorize_backfill_or_movement` asserts
  that it fails and that nothing moved.
- Files on the `cycle2_module_budget` list stay at 300 lines or fewer. Put
  new helpers in a new file under `tests/support/` of 300 lines or fewer,
  rather than lengthening `profile_rotate.rs`.

## Check your test

Run every command below through the same wrapper the pre-push hook uses.
The wrapper runs the command it is given, so name `cargo` first.

For an integration test, run the test by name, then its whole module and
the line budget:

```bash
nice -n 15 ionice -c3 scripts/git-hook-cargo.sh cargo test -p amplihack-cli --test cli_integration \
  issue_1592_oneshot::an_unknown_login_is_adopted_as_default_original
nice -n 15 ionice -c3 scripts/git-hook-cargo.sh cargo test -p amplihack-cli --test cli_integration \
  issue_1592_oneshot cycle2_module_budget
```

For a rotate unit test, run the module's unit tests:

```bash
nice -n 15 ionice -c3 scripts/git-hook-cargo.sh cargo test -p amplihack-cli --lib \
  commands::profile::rotate
```

Then search the diff for real paths and tokens. This is a minimal check,
not a full review:

```bash
git diff origin/main...HEAD | grep -nE 'home_dir\(\)|env::var\("HOME"\)|0\.0\.0\.0|"[^"]*\.claude(\.json|/)|fake-(task|rotation|default)1753|SENTINEL|SECRET-KEY|(println|dbg)!.*accessToken'
```

Each match must be inside a temp-`HOME` fixture (a `.claude` path must be
joined to the fixture's `home` or `root`, not written as a literal), be one
of the fixtures' own fake tokens or sentinel strings used in an assertion,
or be explained in the pull request.

## Related

- [`amplihack profile` reference](../reference/profile-command.md#identity-and-duplicate-logins) - Account identity, recorded accounts and the duplicate-login warning
- [Claude profile rotation](./claude-profile-rotation.md) - How rotation uses login-bound identity
- [Keep Tests Passing Under Host Load](./keep-tests-passing-under-host-load.md#rotator-tests) - The identity knobs `Fixture::base` sets, and how to test budget exhaustion
- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - `isolated(..)` and the real-root guard
- [Keep Tests from Changing PATH and HOME](./keep-tests-from-changing-path-and-home.md) - Giving a child its own `HOME`
