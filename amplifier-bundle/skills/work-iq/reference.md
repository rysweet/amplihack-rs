# Work IQ Technical Reference

Complete technical documentation for Microsoft Work IQ MCP server integration.

## Installation Methods

### Method 1: NPX (Recommended)

Run on-demand without installation:

```bash
npx -y @microsoft/workiq mcp
```

### Method 2: Global Install

```bash
npm install -g @microsoft/workiq
workiq version
```

### Method 3: Via MCP Manager

Using amplihack's mcp-manager skill:

```bash
cd .claude/scenarios
python3 -m mcp-manager.cli add workiq npx -- -y @microsoft/workiq mcp
```

## MCP Configuration

### Basic Configuration

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "workiq": {
      "command": "npx",
      "args": ["-y", "@microsoft/workiq", "mcp"]
    }
  }
}
```

### With Specific Tenant

```json
{
  "mcpServers": {
    "workiq": {
      "command": "npx",
      "args": ["-y", "@microsoft/workiq", "mcp"],
      "env": {
        "WORKIQ_TENANT_ID": "your-tenant-id"
      }
    }
  }
}
```

### VS Code Integration

Add to VS Code MCP settings:

```json
{
  "mcp.servers": {
    "workiq": {
      "command": "npx",
      "args": ["-y", "@microsoft/workiq", "mcp"]
    }
  }
}
```

## Authentication

### OAuth Flow

1. **First Query** - Triggers authentication
2. **Browser Opens** - Sign in to Microsoft 365
3. **Grant Permissions** - Accept requested permissions
4. **Token Cached** - Subsequent queries use cached credentials

### Admin Consent

Enterprise deployments may require tenant administrator consent:

1. **User Attempts Access** - Consent dialog appears
2. **Admin Reviews** - Evaluates permission request
3. **Consent Granted** - Admin approves via Entra admin center
4. **Access Enabled** - All users can authenticate

**Admin Consent URL:**

```
https://entra.microsoft.com/ → Enterprise applications → WorkIQ → Permissions → Grant admin consent
```

### Required Permissions

| Permission                | Type      | Purpose                |
| ------------------------- | --------- | ---------------------- |
| `Mail.Read`               | Delegated | Read email messages    |
| `Calendars.Read`          | Delegated | Access calendar events |
| `ChannelMessage.Read.All` | Delegated | Read Teams messages    |
| `Files.Read.All`          | Delegated | Search documents       |
| `People.Read`             | Delegated | Find people info       |

### Clear Credentials

```bash
# Remove cached tokens - platform specific locations:
# Windows: %LOCALAPPDATA%\Microsoft\WorkIQ
# macOS: ~/Library/Application Support/Microsoft/WorkIQ
# Linux: ~/.config/Microsoft/WorkIQ
```

## CLI Command Reference

### workiq accept-eula

Accept the End User License Agreement. **Required before first use.**

```bash
workiq accept-eula
# or
npx @microsoft/workiq accept-eula
```

### workiq ask

Query M365 data interactively or with a specific question.

```bash
# Interactive mode
workiq ask

# Direct question
workiq ask -q "What meetings do I have tomorrow?"

# Specific tenant
workiq ask -t "your-tenant-id" -q "Show my emails"
```

### workiq mcp

Start MCP stdio server for agent communication.

```bash
workiq mcp
# Runs as daemon, invoked by MCP runtime
```

### workiq version

Display version information.

```bash
workiq version
```

## Query Patterns

### Date Ranges

| Syntax       | Interpretation   |
| ------------ | ---------------- |
| "today"      | Current day      |
| "yesterday"  | Previous day     |
| "this week"  | Current week     |
| "last week"  | Previous 7 days  |
| "last month" | Previous 30 days |

### Query Types

| Data Type | Example Query                                    |
| --------- | ------------------------------------------------ |
| Email     | "What did John say about the proposal?"          |
| Calendar  | "What's on my calendar tomorrow?"                |
| Teams     | "Messages in Engineering channel about deadline" |
| Documents | "Find my recent PowerPoint presentations"        |
| People    | "Who is working on Project Alpha?"               |

## Troubleshooting

### EULA Not Accepted

```
Error: EULA not accepted
```

**Solution:**

```bash
npx @microsoft/workiq accept-eula
```

### Authentication Failed

```
Error: Authentication failed
```

**Solutions:**

1. Clear cached credentials (see locations above)
2. Verify M365 account is active
3. Check network connectivity
4. Try re-authenticating

### Admin Consent Required

```
Error: Admin consent required for this application
```

**Solutions:**

1. Contact M365 tenant administrator
2. Request application consent
3. Admin grants via Entra admin center

### WSL Browser Issues

```
Error: Unable to open browser for authentication
```

**Solution:**

```bash
sudo apt install xdg-utils
sudo apt install wslu
```

### Rate Limiting

```
Error: Rate limit exceeded
```

**Solution:**

- Wait for cooldown period (usually 60 seconds)
- Reduce query frequency
- Use more specific queries

## Using WorkIQ from WSL through Windows

There are two routes on WSL2. Try the native route first.

1. **Native route.** Run the WorkIQ MCP server inside WSL as described above. If the browser sign-in does not open, apply the fix under [WSL Browser Issues](#wsl-browser-issues).
2. **Windows bridge.** If the native sign-in still does not work, send each query through `powershell.exe` to the Copilot CLI installed on the Windows host, with the WorkIQ plugin. Sign-in then happens in a browser on Windows.

The rest of this section describes the Windows bridge. It is a set of command lines; no script is shipped.

### Prerequisites

1. **WSL2.** The bridge needs WSL2 with interop enabled, not WSL1 or native Linux.

   ```bash
   grep -qi microsoft /proc/version && echo "WSL detected" || echo "Not WSL"
   ```

2. **Copilot CLI on the Windows host.** Install it from Windows Terminal or PowerShell, not inside WSL:

   ```powershell
   winget install GitHub.Copilot
   ```

   Check it from WSL:

   ```bash
   powershell.exe -NoProfile -Command "copilot --version" 2>&1
   ```

3. **GitHub sign-in for the Windows Copilot CLI:**

   ```bash
   powershell.exe -NoProfile -Command "copilot login" 2>&1
   ```

4. **The WorkIQ plugin in the Windows Copilot CLI:**

   ```bash
   powershell.exe -NoProfile -Command "copilot plugin install workiq" 2>&1
   ```

5. **M365 sign-in.** WorkIQ asks for Microsoft sign-in on first use. A browser opens on the Windows host. Approve the consent prompt.

### How the Bridge Works

```text
WSL session
     |
     v
powershell.exe -NoProfile -Command "copilot -p '...' --allow-all-tools --silent --no-auto-update 2>&1"
     |
     v
Windows Copilot CLI (native)
     |
     v
WorkIQ MCP server (npx @microsoft/workiq@latest mcp)
     |
     v
Microsoft Graph API -> M365 (emails, calendar, files, Teams)
```

Each query starts a `powershell.exe` process that runs the Windows Copilot CLI with a natural-language prompt. The prompt asks Copilot to use the `ask_work_iq` tool of the `workiq` server:

```bash
powershell.exe -NoProfile -Command "copilot -p 'Use the workiq ask_work_iq tool to answer: <QUESTION>' --allow-all-tools --silent --no-auto-update 2>&1"
```

| Flag                 | Purpose                                                 |
| -------------------- | ------------------------------------------------------- |
| `--allow-all-tools`  | Lets Copilot call the WorkIQ tool without approval      |
| `--silent`           | Suppresses interactive UI elements                      |
| `--no-auto-update`   | Skips update checks that slow each query down           |
| `2>&1`               | Captures both standard output and errors from Windows   |

Take the answer out of the Copilot CLI output and present it on its own; the output also contains status messages and formatting.

### Detecting WSL and the Bridge

Check these in order before using the bridge:

1. **WSL.** `grep -qi microsoft /proc/version`. If this is not WSL, use the native route.
2. **Windows Copilot CLI.** `powershell.exe -NoProfile -Command "copilot --version" 2>&1`. If it is missing, install it (prerequisite 2).
3. **WorkIQ plugin.** `powershell.exe -NoProfile -Command "copilot plugin list" 2>&1 | grep -i workiq`. If it is missing, install it (prerequisite 4).
4. **Route the query** with the command above.

If something is missing, the setup in short is:

```text
1. Install Copilot CLI on Windows: winget install GitHub.Copilot
2. Sign in: powershell.exe -NoProfile -Command "copilot login"
3. Install WorkIQ: powershell.exe -NoProfile -Command "copilot plugin install workiq"
4. Run the query again. WorkIQ asks for M365 sign-in on first use.
```

### Bridge Troubleshooting

| Problem                           | Cause                                         | Solution                                                                                                       |
| --------------------------------- | --------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `copilot: command not found`      | Copilot CLI not installed on the Windows host | Run `winget install GitHub.Copilot` from Windows Terminal                                                      |
| `powershell.exe: not found`       | Not WSL2, or WSL interop disabled             | Check that `/proc/sys/fs/binfmt_misc/WSLInterop` exists                                                        |
| WorkIQ timeout or no response     | M365 sign-in expired or never completed       | Run the query again to start the browser sign-in again                                                         |
| PowerShell execution policy error | Restrictive execution policy                  | `-NoProfile` usually avoids it; otherwise run `Set-ExecutionPolicy RemoteSigned -Scope CurrentUser` in PowerShell |
| Quote escaping problems           | Nested quotes between bash and PowerShell     | Use double quotes for the outer PowerShell command and single quotes for the inner prompt                      |
| Garbled output or ANSI codes      | Terminal encoding mismatch                    | Pipe the output through `sed 's/\x1b\[[0-9;]*m//g'`                                                             |
| `GitHub auth required`            | Windows Copilot CLI not signed in             | Run `powershell.exe -NoProfile -Command "copilot login"`                                                       |
| Slow responses (30 s or more)     | Each query starts Copilot and the MCP server  | Expected; the first query is the slowest                                                                       |

### Bridge Limitations

- **Write actions.** WorkIQ can query M365 data and draft content. Sending email, creating documents and posting Teams messages may need further admin consent or Graph API permissions.
- **Windows host required.** The bridge works only in WSL on a Windows host with Copilot CLI installed. It does not work on native Linux or macOS.
- **No session memory.** Each query starts a new Copilot CLI process, so nothing is kept between queries.
- **Start-up time.** Each query includes Copilot CLI start-up and WorkIQ MCP server start-up, about 10 to 30 seconds.
- **No streaming.** The whole answer arrives when the Windows process ends.
- **Scope.** Access is limited to what the M365 account may read. Some tenants require admin consent.

## Security Considerations

### Data Access

- Queries scoped to YOUR M365 account
- Respects M365 permissions
- No data sent to third parties (except Microsoft)

### Token Storage

- Platform-standard secure storage:
  - Windows: Credential Manager
  - macOS: Keychain
  - Linux: Secret Service API

### Best Practices

- Review permissions before granting consent
- Use least-privilege principle
- Revoke access when no longer needed
- Regular access reviews for enterprise

---

**Last Updated:** 2026-01-23
