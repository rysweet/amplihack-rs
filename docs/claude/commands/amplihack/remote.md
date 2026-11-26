---
name: amplihack:remote
disable-model-invocation: true
version: 1.0.0
description: Execute amplihack commands on remote Azure VMs using azlin
triggers:
  - "Run on remote VM"
  - "Execute remotely"
  - "Use Azure VM"
---

# Remote Execution Command

Execute amplihack commands on remote Azure VMs using azlin for provisioning.

## Usage

```bash
/amplihack:remote [OPTIONS] COMMAND PROMPT
```

Run `amplihack remote exec [OPTIONS] COMMAND PROMPT` with the supplied arguments.

## Arguments

- **COMMAND**: Amplihack command to execute
  - `auto` - Autonomous coding mode

Only `auto` is supported for execution. The parser also accepts `ultrathink`,
`analyze` and `fix`, but execution fails for those modes.

- **PROMPT**: Task description (required)

## Options

- `--max-turns N` - Maximum turns for auto mode (default: 10, range: 1-50)
- `--vm-size SIZE` - Azure VM size (default: Standard_D2s_v3)
- `--vm-name NAME` - Reuse specific VM by name
- `--keep-vm` - Don't cleanup VM after execution (for debugging)
- `--no-reuse` - Always provision fresh VM (skip reuse logic)
- `--timeout MINUTES` - Maximum time without output in minutes (default: 120, range: 5-480)
- `--region REGION` - Azure region (default: from azlin config)

## Examples

### Basic Usage

Execute auto mode with default settings:

```bash
/amplihack:remote auto "implement user authentication"
```

### With Options

Execute with custom max turns:

```bash
/amplihack:remote --max-turns 20 auto "refactor the API module"
```

Keep VM for debugging:

```bash
/amplihack:remote --keep-vm auto "analyse performance bottlenecks"
```

Use specific VM size for compute-intensive tasks:

```bash
/amplihack:remote --vm-size Standard_D4s_v3 auto "process large dataset"
```

### Advanced Usage

Reuse a specific VM:

```bash
/amplihack:remote --vm-name amplihack-user-20251120-143022 auto "continue previous work"
```

Force fresh VM (no reuse):

```bash
/amplihack:remote --no-reuse auto "start clean implementation"
```

Set the idle timeout for long-running tasks:

```bash
/amplihack:remote --timeout 240 auto "comprehensive refactoring"
```

## Workflow

The command executes the following workflow:

1. **Validate Environment**
   - Check azlin installation before provisioning
   - Verify git repository
   - Validate .claude directory during packaging

2. **Package Context**
   - Scan for hardcoded secrets (fails if found)
   - Create git bundle with all branches
   - Archive .claude directory
   - Create combined context.tar.gz

3. **Provision VM**
   - Check for reusable VMs (if `--no-reuse` not set)
   - Provision new VM via azlin if needed
   - Wait for VM to be ready

4. **Transfer Context**
   - Upload context archive to VM

5. **Execute Remote Command**
   - Extract context on VM
   - Restore git repository
   - Use amplihack already installed on the VM
   - Run specified command with prompt
   - Monitor execution with timeout

6. **Retrieve Results**
   - Download execution logs
   - Fetch git branches and commits
   - Copy retrieved logs to `.claude/runtime/logs/remote/` in the local repository

7. **Cleanup**
   - Delete VM after success or an idle timeout (unless `--keep-vm` set)
   - Clean up temporary files
   - Display integration summary

## Result Integration

Remote results are imported into the `remote-exec/` git namespace:

```bash
# View remote branches
git branch -r | grep remote-exec

# Merge remote changes
git merge remote-exec/main

# Cherry-pick specific commits
git cherry-pick remote-exec/feature-branch
```

## Security

### Secret Scanning

The command performs comprehensive secret scanning before packaging:

**Detected Patterns**:

- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`
- Generic API keys (sk-ant-_, sk-_, ghp\_\*)
- Azure/AWS credentials
- Passwords and tokens

**Files Excluded from Scanning**:

- `.env*` files
- SSH keys (`.ssh/`, `*.pem`, `*.key`)
- Cloud credentials (`.aws/`, `.azure/`)
- `node_modules/`, `__pycache__/`, `.venv/`

**Hard Requirement**: Command fails immediately if secrets detected.

These exclusions apply to scanning, not to the git bundle or the `.claude`
archive. Remove sensitive content from tracked history and from `.claude` before
packaging it.

### API Key Transfer

The command reads `ANTHROPIC_API_KEY` and sends it over the SSH command's standard
input, followed by the prompt. The remote script exports it for amplihack.

## VM Management

### VM Reuse Logic

By default, the command tries to reuse existing VMs matching:

- Name: Starts with `amplihack-`
- Age: At most 24 hours when creation time is available
- Size: Matches requested `--vm-size`

Reuse does not check whether a VM is already running another task.

### VM Naming

VMs are named using pattern: `amplihack-{username}-{timestamp}`

Example: `amplihack-user-20251120-143022`

### Manual VM Management

List VMs:

```bash
azlin list
```

Connect to VM:

```bash
azlin connect amplihack-user-20251120-143022
```

Delete VM manually:

```bash
azlin kill amplihack-user-20251120-143022
```

## Error Handling

### Packaging Errors

**Secret Detected**:

```
ERROR: Secret detected during packaging

File: src/config/api.py
Line: 15
Content: ANTHROPIC_API_KEY = "sk-ant-1234..."

Action required:
  1. Remove hardcoded API key from source code
  2. Supply credentials outside tracked history and the .claude archive
  3. Retry command
```

**Solution**: Remove secrets, add to `.env` file, retry.

### Provisioning Errors

**Azlin Not Installed**:

```
ERROR: Azlin not found. Please install:
  cargo install azlin
```

**Solution**: Install azlin and configure Azure access as its documentation describes.

### Execution Errors

**Timeout**:

```
Reaped after 7200s with no output (idle window)
```

**Solution**: Increase the idle timeout or use `--keep-vm` to retain the VM for inspection.

### Integration Errors

**Merge Conflicts**:

```
WARNING: Conflicts detected!
Branch 'main' has diverged: local=a1b2c3d4, remote=e5f6g7h8

Branches available in 'remote-exec/' namespace for manual merge:
  git merge remote-exec/main
```

**Solution**: Manually merge using git commands.

## Troubleshooting

### VM Provisioning Takes Too Long

- Check Azure subscription status
- Verify region availability
- Try different VM size
- Check Azure quota limits

### Transfer Fails

- Check network connection
- Verify archive size < 500MB
- Try again (automatic retry included)
- Check Azure VM disk space

### Execution Fails

Non-zero command exits preserve the VM. For idle timeouts, use `--keep-vm` to
prevent cleanup. Inspect a retained VM:

```bash
azlin connect amplihack-user-20251120-143022
cd ~/workspace
ls .claude/runtime/logs/
```

### Results Not Retrieved

Retrieval failure alone does not preserve the VM. Use `--keep-vm` if manual
retrieval may be needed:

```bash
# Connect to VM
azlin connect amplihack-user-20251120-143022

# Create bundle and logs in home directory (azlin cp requires ~/ paths)
cd ~/workspace
git bundle create ~/results.bundle --all
tar czf ~/logs.tar.gz .claude/runtime/logs/

# Download locally (azlin cp uses session:path notation)
azlin cp amplihack-user-20251120-143022:~/results.bundle results.bundle
azlin cp amplihack-user-20251120-143022:~/logs.tar.gz logs.tar.gz
```

## Requirements

### Local Machine

- **azlin**: Installed (`cargo install azlin`) with Azure access configured
- **Git** and **tar**: Available on the local machine
- **Azure CLI**: Available as required by azlin
- **ANTHROPIC_API_KEY**: Set in environment
- **Git repository**: Includes a `.claude` directory; commit source changes before
  execution because the git bundle includes committed history, not uncommitted edits

### Azure Subscription

- Active subscription with credits
- Sufficient VM quota (Standard_D series)
- Network access to Azure regions

### Remote VM

- **amplihack**, **Git** and **tar**: Already installed and available on the VM

## Cost Considerations

### VM Costs

Estimated costs (as of 2025):

- **Standard_D2s_v3**: ~$0.10-0.15/hour (2 vCPU, 8GB RAM)
- **Standard_D4s_v3**: ~$0.20-0.30/hour (4 vCPU, 16GB RAM)

### Cost Optimization

1. **VM Reuse**: Automatically reuses VMs at most 24 hours old
2. **Auto-Cleanup**: VMs deleted after success or an idle timeout unless `--keep-vm` is set
3. **Timeout**: Stops execution after the configured period without output
4. **Keep-VM Flag**: Use sparingly (for debugging only)

### Monitor Costs

```bash
# List running VMs
azlin list

# Check VM age
azlin list | grep amplihack

# Cleanup old VMs
azlin kill amplihack-user-OLD-TIMESTAMP
```

## Logs

All remote execution logs are stored in:

```
.claude/runtime/logs/remote/
```

Log files include:

- Execution stdout/stderr
- Git operations
- Integration summary
- Error details

## Best Practices

1. **Always check for secrets**: Review tracked history and `.claude` before packaging
2. **Use VM reuse**: Saves 5-7 minutes provisioning time
3. **Set appropriate idle timeouts**: Stop runs that produce no output
4. **Preserve VMs on failure**: Use `--keep-vm` for debugging
5. **Clean up manually**: Check `azlin list` periodically
6. **Monitor costs**: Track VM usage and Azure bills
7. **Test locally first**: Validate commands work before remote execution
8. **Use .env for secrets**: Never hardcode in source files

## See Also

- `/amplihack:auto` - Local autonomous mode
- `/amplihack:ultrathink` - Local deep analysis mode
- Azlin documentation: https://github.com/rysweet/azlin
- Azure VM pricing: https://azure.microsoft.com/pricing/
