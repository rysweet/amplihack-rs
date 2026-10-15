# Review: GG-Santos/ccswitch

This review records what amplihack adopted from
[GG-Santos/ccswitch](https://github.com/GG-Santos/ccswitch) when it added
[Claude login profiles](../howto/claude-profiles.md), and what it rejected.
No ccswitch code was copied; amplihack's implementation is independent and
is built only on Claude Code's documented `CLAUDE_CONFIG_DIR` variable.

## Adopted

| Idea | amplihack form |
|------|----------------|
| Named accounts, one per login | Named profiles, each an isolated directory `<root>/<name>/` |
| Quick switching between accounts | `amplihack profile switch <name>` writes a one-line pointer file |
| Log in once per account | `amplihack profile login <name>` runs interactive `claude` under that directory |
| Run as a specific account | `amplihack profile run <name> -- <args>` and `--profile <name>` on `launch`/`claude` |

Isolation comes from Claude Code itself: it reads credentials, settings and
history from `CLAUDE_CONFIG_DIR`. amplihack sets that variable on the child
process only.

## Rejected

| ccswitch-style feature | Why amplihack does not do it |
|------------------------|------------------------------|
| Shared auth-token vault / swapping credentials in `~/.claude` | Mutates the user's real Claude state and concentrates secrets. amplihack never opens, copies or logs credential files. |
| Direct token refresh | Reimplements Anthropic's auth flow; fragile and a credential-handling risk. Claude refreshes its own tokens. |
| Usage / quota probing | Requires reading tokens and calling private endpoints. No network access is added. |
| Background auto-rotation between accounts | Silently changes which account is billed and may violate the provider's terms. Switching is always an explicit manual action. |
| Credential import/export | Out of scope for v1. |

## Resulting safety properties

- Profile names are allowlist-validated; they can't traverse out of the root.
- Symlinked profile directories are refused.
- Profile directories are mode `0700` on Unix; the pointer file is `0600`.
- `remove` needs `--yes` and only deletes a verified directory inside the root.
- A dangling or corrupt default pointer is an error, never a silent fallback.
- Profiles are **not** a security boundary against the same OS user.
