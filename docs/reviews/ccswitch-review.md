# Review: GG-Santos/ccswitch

Reviewed upstream commit [`b5a2dd64`](https://github.com/GG-Santos/ccswitch/tree/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0) on 2026-10-15. This records the implementation behind the account-switching idea and the decisions used for [Amplihack login profiles](../howto/claude-profiles.md). No upstream code was copied or executed with real credentials.

This was a static review of the switching, storage, refresh, usage and watcher paths. The upstream test suite was not run.

## How upstream switching works

| Component | Behavior at the reviewed revision |
|-----------|----------------------------------|
| [`creds.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/creds.py) | Reads `claudeAiOauth` from the live configuration directory's `.credentials.json`. Switching replaces that subtree, preserves other JSON keys, backs up the previous file, and publishes via a flushed temporary file and `os.replace`. Backups are bounded to ten. |
| [`vault.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/vault.py) | Stores named credential blobs, the active account and rotation order in `~/.cc-accounts/vault.json` or `CCSWITCH_HOME`. Read/modify/write transactions use an OS file lock. Vault publication also uses a temporary file and replacement. |
| [`crypto.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/crypto.py) | Protects stored token blobs using Windows DPAPI when available. This is platform-dependent; the reviewed non-Windows path stores plaintext behind filesystem permissions. |
| [`actions.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/actions.py) | Checkpoints the active account's latest credentials before a switch, because Claude rotates refresh tokens. It can refresh the target before taking the vault lock, saves the new active account, then writes the live credentials file. |
| [`oauth.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/oauth.py) | Calls the provider's OAuth token endpoint with a stored refresh token and client ID. It refreshes within a five-minute expiry buffer and saves rotated token values. |
| [`usage.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/usage.py) | Usage measurement makes an actual `/v1/messages` inference request with `max_tokens: 1`. It derives utilization from rate-limit response headers, with short memory/disk caches. This consumes account usage; it is not a passive local-file inspection. |
| [`selection.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/selection.py), [`daemon.py`](https://github.com/GG-Santos/ccswitch/blob/b5a2dd64da30f891cf82e1f1cf595f09f03de9b0/ccswitch/daemon.py) | The watcher polls the active account, defaults to a 90% threshold, then probes candidates when switching is needed. Selection prefers usable headroom; when all candidates are exhausted, it considers an earlier reset with a margin to avoid churn. Poll timing adapts to utilization and resets. |

## Adopted now (#1569)

The first version of [login profiles](../howto/claude-profiles.md) adopted
none of ccswitch's switching. Issue #1569 adds
[auto-switch](../howto/claude-profiles.md#switch-accounts-automatically),
which takes ccswitch's goal (keep working when one account runs out) and
some of its policy, but not its mechanism. In summary:

- **Passive status-line meter.** Usage comes from the `rate_limits` Claude
  Code already hands the status line, because measuring should not spend
  quota or need tokens.
- **Move-not-copy swaps under a ledger and a lease.** Each login is in one
  place, and each account is live in at most one directory. Claude rotates
  refresh tokens, so any stored copy goes stale.
- **A journal for crash recovery.** A swap that is cut short is rolled back
  or forward at the next launch or check, so a crash cannot lose a login or
  leave one in two places.
- **An environment variable that overrides the file.**
  `AMPLIHACK_CLAUDE_AUTO_SWITCH` takes precedence over `.auto-switch`.
  Switching is opt-in and can be forced per shell. The file is read on every
  check, so turning it off takes effect in running sessions unless the
  variable forces it on.
- **Substitution for nested launches, and reruns after a usage limit.**
  Recipe steps and workstreams start under a profile that has quota left,
  and a step that hits a usage limit is rerun under the next profile. Work
  keeps going without moving any logins, and finished steps are not repeated.

The table compares each mechanism with ccswitch's.

| ccswitch | amplihack auto-switch | Why |
|----------|-----------------------|-----|
| Copies credential blobs into a vault and checkpoints them before each switch, because Claude rotates refresh tokens. | **Moves** `.credentials.json` with `renameat2(RENAME_NOREPLACE)`. Each account's login is in exactly one place: its own profile, a private parked slot, or the one directory where it is live. Nothing is copied. | Checkpointing exists to repair the damage copying does: any stored copy goes stale at the next refresh. A move cannot leave a stale copy behind, so there is nothing to checkpoint. |
| Vault transaction and live-file write are two separate atomic steps; concurrent switches can interleave them. | One global file lock covers every move, a journal records each swap in progress, and recovery works out the state from which of two known paths each file is at. | Makes the pair of moves one recoverable transaction, closing the interleaving noted in the original review. |
| Calls the OAuth token endpoint to refresh tokens itself. | Never refreshes, reads or parses a token. During a swap it holds Claude's own refresh lock (`.oauth_refresh.lock`) so Claude cannot write back a token mid-move, and sets the moved file's modification time so Claude reloads it. | Claude remains the only process that refreshes a token, so there is no second token owner. |
| Measures usage with a real `/v1/messages` request per account (`max_tokens: 1`), which spends quota. | A **passive meter**: a status-line wrapper records the `rate_limits` Claude Code already passes to the status line, then runs the user's own status line unchanged. No probes and no network calls. | Measuring should not consume what it measures, and amplihack should not hold the tokens a probe would need. Accounts that have not run a session since the switch was turned on have no reading and are tried last. |
| A background daemon polls the active account. | The status-line wrapper is the trigger. It runs after each Claude turn, as a child of the running session, so it sees that session's numbers and its `CLAUDE_CONFIG_DIR`. Session end and crash recovery run at every launch, login, status-line update and `auto-switch repair`; `auto-switch status` only reports what they would do. | No long-running process to install or supervise, and the check happens exactly when fresh numbers arrive. |
| 90% default threshold; prefers the candidate with most headroom; considers earlier resets. | Same 90% default, on either the five-hour or the seven-day window; picks the lowest usage; a window whose reset time has passed counts as 0%. | This policy was sound and simple. The churn margin was not adopted; instead a directory is not swapped again for five minutes, and readings that still describe the previous account are ignored. |
| Switching runs whenever the watcher daemon is running. | Off by default. `amplihack profile auto-switch on\|off`, with `AMPLIHACK_CLAUDE_AUTO_SWITCH=1\|0` taking precedence per shell. The file is read on every check; a session's variable is fixed when it starts. An exclude list keeps chosen profiles from ever being targets. | Moving logins is security-sensitive, so it must be opt-in, and turning it off must reach running sessions that the variable does not force on. |
| Switches the single live `~/.claude`. | Works only inside profile directories under the profile root and never touches `~/.claude`. Only sessions started under a profile can be swapped. | Keeps the user's default Claude setup out of scope. |

Two further pieces have no ccswitch equivalent. Nested launches (recipe
steps, workstreams) that would inherit an exhausted profile start under one
with quota left; they never move logins. A recipe step that fails with a
usage limit is rerun under the next profile at the step level, inside the
step's own `amplihack claude -p` launch, not by rerunning the recipe,
because rerunning finished steps would repeat their commits and pull
requests. It uses the recipe runner's existing `usage_limit` classifier, so
the two never disagree about what a usage limit is.

ccswitch identifies accounts by their stored OAuth identity. amplihack uses
the name of the profile that owns the login, which needs no token material;
the `oauthAccount` block in `.claude.json` is only patched so `/status`
shows the right name, and Claude can briefly overwrite that patch.

The design depends on two behaviours expected of Claude Code versions that
pass `rate_limits` to the status line (credential reload on a
modification-time change, and the refresh lock). They have not been
confirmed with two real accounts, so the switch is off by default and the how-to gives a
[manual check](../howto/claude-profiles.md#check-it-with-two-real-accounts).

## Not adopted

| Feature | Why not |
|------------------|---------|
| Active usage probing (`/v1/messages` with `max_tokens: 1`) or any other network call | It spends the quota it measures and needs amplihack to hold tokens. The passive status-line reading is enough. |
| A token vault, token encryption, or any copy of a credential | A copy goes stale at the next refresh token rotation. Moving the one file is simpler and cannot lose a login. |
| Refreshing tokens or calling OAuth endpoints | Claude stays the only process that refreshes a token. |
| The interactive UI for picking accounts | `profile switch`, `--profile` and `auto-switch status` already cover choosing and inspecting accounts from the CLI. |
| Syncing accounts between machines | It would mean transmitting credentials, which amplihack never does. |
| A background watcher process | The status-line wrapper already runs exactly when fresh numbers arrive. |
| Switching `~/.claude` or the macOS Keychain | `~/.claude` is the user's default setup and out of scope; the Keychain has no file that can be moved. |
| Switching other agent backends (Copilot, Codex) | They have no equivalent `rate_limits` reading or profile directories. |

## Authentication scope and validation

Claude's [official multiple-account documentation](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts) supports separate directories for claude.ai logins and API keys. It also documents an exception: separate directories do not isolate two Claude Console sign-ins made **without an API key**, because that authentication lives outside the configuration directory.

Inherited authentication variables still apply. In particular, an exported `ANTHROPIC_API_KEY` can select API billing instead of a subscription login, as explained in the [environment reference](https://code.claude.com/docs/en/env-vars). Amplihack preserves the user's environment rather than silently changing their authentication method.

Automated CLI acceptance uses isolated homes, an observable Claude fixture and sentinel credential files to verify profile routing, asset copying, switching, removal, preservation of the shared source, and for auto-switch: move-not-copy, one location per account, lease refusals, crash recovery and that the off state changes nothing. It does not prove a second real OAuth login. To validate real accounts, log in through each named profile and check `/status` inside each Claude session; no credentials need to be shared with the test runner.
