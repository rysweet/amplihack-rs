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

## Engineering implications

Token checkpointing is essential when credentials are copied into a vault: retaining an old refresh token can make a saved account unusable. Delegating refresh to the Claude process within a persistent profile avoids that extra token owner.

The vault and live credential file are separate resources. In the reviewed `switch_to`, the vault transaction ends before `write_live_oauth`. Consequently, atomic replacement of each file does not make the pair one transaction. Concurrent switches can interleave those two publications; this is a source-level inference, not a reproduced upstream incident.

Automatic selection also needs a policy for unknown usage, unavailable accounts, reset times and which account pays for the next request. Its probes require token access and inference requests. Amplihack's first version provides explicit profile selection and does not introduce a token vault, refresh client, quota probe or background watcher.

## Amplihack's directory-based design

Each named profile is a persistent configuration directory. `profile login` launches Claude against that directory, `profile run` selects it for one child, and `profile switch` records a default for future launches. An already-running Claude session keeps its selected directory. Amplihack changes the child's `CLAUDE_CONFIG_DIR`; it does not swap the user's live `~/.claude` or copy credentials.

New profiles receive only the documented shared-asset/settings allowlist. Copying uses private staging followed by publication without replacing an existing profile. Filesystem checks, copy limits, permission checks and pointer validation are described in the [command reference](../reference/profile-command.md). Profiles are not a security boundary against another process running as the same OS user.

## Authentication scope and validation

Claude's [official multiple-account documentation](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts) supports separate directories for claude.ai logins and API keys. It also documents an exception: separate directories do not isolate two Claude Console sign-ins made **without an API key**, because that authentication lives outside the configuration directory.

Inherited authentication variables still apply. In particular, an exported `ANTHROPIC_API_KEY` can select API billing instead of a subscription login, as explained in the [environment reference](https://code.claude.com/docs/en/env-vars). Amplihack preserves the user's environment rather than silently changing their authentication method.

Automated CLI acceptance uses isolated homes and an observable Claude fixture to verify profile routing, asset copying, switching, removal and preservation of the shared source. It does not prove a second real OAuth login. To validate real accounts, log in through each named profile and check `/status` inside each Claude session; no credentials need to be shared with the test runner.
