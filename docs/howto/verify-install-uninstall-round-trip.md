# How to Verify the Install/Uninstall Round-Trip

This guide walks through running a full install → uninstall → reinstall cycle
on a fresh VM to confirm that `amplihack install` and `amplihack uninstall`
are clean and idempotent with no leftover state.

Run this procedure before releasing a new version of the CLI.

## Prerequisites

- `azlin` installed and authenticated (`azlin vm list` works without error)
- `gh` authenticated (`gh auth status` shows an active account)
- Azure CLI authenticated (`az account show` returns a valid subscription)
- A published `amplihack` release binary for `x86_64-unknown-linux-gnu`
  (check [releases](https://github.com/rysweet/amplihack-rs/releases))

Run these pre-flight checks before any VM operations. Abort if any fails —
otherwise Azure errors will be confusing:

```sh
# Verify GitHub authentication
gh auth status

# Verify Azure authentication — must return a subscription JSON object
az account show

# Verify SSH key permissions — must print 600
chmod 600 "$SSH_KEY_PATH"
stat -c "%a" "$SSH_KEY_PATH"
```

> Set `SSH_KEY_PATH` to the path printed by `azlin vm create` in Step 1.

---

## Step 1: Create a Fresh VM

```sh
azlin vm create --size Standard_D2s_v3
```

`azlin` prints the VM name, IP address, and SSH key path. Save the IP for the
steps below. The VM is a clean Ubuntu image with no pre-installed tooling.

If you do not have `azlin`, list available VMs and pick one that was not
recently modified:

```sh
azlin vm list
```

---

## Step 2: Bootstrap the VM

SSH into the VM and install the Rust toolchain plus the amplihack binary.

```sh
# Install Rust (stable)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

### Download binary (never pipe directly to shell)

Always download to a named file first:

```sh
curl -sSfL \
  https://github.com/rysweet/amplihack-rs/releases/latest/download/amplihack-x86_64-unknown-linux-gnu.tar.gz \
  -o /tmp/amplihack.tar.gz
```

### Verify checksum (mandatory)

Attempt to download the `.sha256` file. Take the appropriate branch:

**Branch A — `.sha256` file is present (preferred):**

```sh
curl -sSfL \
  https://github.com/rysweet/amplihack-rs/releases/latest/download/amplihack-x86_64-unknown-linux-gnu.tar.gz.sha256 \
  -o /tmp/amplihack.tar.gz.sha256

sha256sum --check /tmp/amplihack.tar.gz.sha256
# Expected output: /tmp/amplihack.tar.gz: OK
```

If `sha256sum --check` exits non-zero, **stop**. Do not install the binary.
Re-download and check again. If the mismatch persists, file a security report.

**Branch B — no `.sha256` file (supply-chain risk):**

If the release does not include a `.sha256` file, record the following finding
in the test report before continuing:

```
SUPPLY-CHAIN RISK: SHA-256 file absent for release <VERSION>.
Supply-chain integrity unverified. Proceeding with download-to-file
mitigation only. File a follow-up issue to add checksum publishing.
```

Do not skip installation — proceed, but document the finding.

### Install the binary

```sh
tar -xzf /tmp/amplihack.tar.gz -C /tmp
chmod +x /tmp/amplihack
sudo mv /tmp/amplihack /usr/local/bin/amplihack

# Verify the binary runs
amplihack --version

# Clone the source repo (required for --local install)
git clone https://github.com/rysweet/amplihack-rs ~/src/amplihack-rs
```

---

## Step 3: Run the Install/Uninstall Cycle

Execute each step in order and capture output. Any non-zero exit code is a
failure.

### 3a. First install

```sh
cd ~/src/amplihack-rs
amplihack install --local .
```

Expected: exit code `0` and all `✓` lines in the summary (7 hooks registered,
manifest written).

### 3b. Verify with doctor

```sh
amplihack doctor
```

Expected: all 7 checks show `✓`. Any `✗` line indicates the install is
incomplete — see [Diagnose with doctor](./diagnose-with-doctor.md).

### 3c. Uninstall

```sh
amplihack uninstall
```

Expected: exit code `0` and a summary showing files, directories, binaries,
and hook registrations removed.

### 3d. Inspect settings.json

```sh
cat ~/.claude/settings.json
```

A clean post-uninstall `settings.json` must satisfy **all** of these:

| Check | Expected |
|-------|----------|
| No array values that are empty `[]` | Prune or remove empty hook arrays |
| No string values containing `amplihack-hooks` | All amplihack hook commands removed |
| No string values containing `tools/amplihack/` | All amplihack tool paths removed |
| Non-amplihack entries intact | XPIA hooks and other tools must survive |

If the file does not exist after uninstall, that is also a valid clean state —
the file is only present when at least one tool has registered a hook.

### 3e. Reinstall

```sh
amplihack install --local ~/src/amplihack-rs
```

Expected: same exit code `0` and `✓` summary as step 3a. Install must be
idempotent — hooks may be updated in-place but must not be duplicated.

### 3f. Final doctor check

```sh
amplihack doctor
```

Expected: all 7 checks show `✓` again, identical to step 3b.

---

## Step 4: Delete the VM

Delete the VM regardless of whether any step failed:

```sh
azlin vm delete <vm-name>
```

Verify the VM is gone:

```sh
azlin vm list
```

---

## What to Do If a Step Fails

| Failure | Next step |
|---------|-----------|
| `amplihack --version` exits non-zero | Check binary architecture; re-download |
| `amplihack install` exits non-zero | Collect stderr; check Python is installed (`python3 -c "import amplihack"`) |
| `doctor` shows `✗ hooks installed` after install | Check `~/.claude/settings.json` for the hook entries manually |
| `uninstall` leaves empty arrays in settings.json | File a bug with the full `settings.json` contents (redact API keys first) |
| Reinstall duplicates hook entries | Check `update_hook_paths()` idempotency; run doctor again to confirm |

---

## See Also

- [Install amplihack for the First Time](./first-install.md)
- [Uninstall amplihack](./uninstall.md)
- [Install Manifest reference](../reference/install-manifest.md)
- [amplihack doctor](./diagnose-with-doctor.md)
