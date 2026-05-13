# C# Validator Integration Guide

This guide covers integrating the C# validation tool into various development workflows and environments.

## Table of Contents

1. [Rust CLI (`amplihack cs-validate`)](#rust-cli-amplihack-cs-validate)
2. [Claude Code Integration](#claude-code-integration)
3. [Git Hooks Integration](#git-hooks-integration)
4. [CI/CD Integration](#cicd-integration)
5. [IDE Integration](#ide-integration)
6. [Custom Workflows](#custom-workflows)
7. [Troubleshooting](#troubleshooting)

---

## Rust CLI (`amplihack cs-validate`)

The recommended way to run C# validation is via the native Rust CLI subcommand. This replaces
the legacy shell/Python scripts with a single statically-linked binary — no Python, no jq,
no external script dependencies.

### Installation

The `cs-validate` subcommand is built into `amplihack`. If you have the CLI installed,
it's already available:

```bash
amplihack cs-validate --help
```

### Usage

```bash
# Validate a single file at the default level (2)
amplihack cs-validate src/Services/MyService.cs

# Validate an entire directory recursively
amplihack cs-validate src/

# Specify validation level
amplihack cs-validate --level 3 src/

# Output as JSON (for CI consumption)
amplihack cs-validate --format json --level 4 src/

# Use a specific config file
amplihack cs-validate --config .claude/config/cs-validator.json src/
```

### Command Reference

```
amplihack cs-validate [OPTIONS] <PATH>

Arguments:
  <PATH>    File or directory to validate (*.cs files discovered recursively)

Options:
  --level <1-4>      Validation level (default: from config or 2)
                       1 = Syntax only (balanced delimiters, patterns)
                       2 = Syntax + dotnet build
                       3 = Syntax + build + Roslyn analyzers
                       4 = All + dotnet format verification
  --config <PATH>    Config file path override
  --format <FORMAT>  Output format: text (default) or json
  -h, --help         Print help
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All validations passed |
| 1 | Validation failed (syntax, build, or analyzer errors) |
| 2 | Configuration error (malformed JSON, invalid level) |
| 3 | Timeout exceeded |
| 4 | Required dependency missing (`dotnet` not found for levels 2-4) |

### Configuration

The CLI searches for configuration in this order:

1. Path passed via `--config`
2. Workspace-local: `.claude/config/cs-validator.json`
3. Global: `~/.amplihack/.claude/config/cs-validator.json`

Config schema:

```json
{
  "enabled": true,
  "validationLevel": 2,
  "analyzerSeverityThreshold": "Error",
  "skipProjects": ["Tests/**/*.csproj"],
  "timeoutSeconds": 30,
  "parallel": true,
  "cacheEnabled": true,
  "reporting": {
    "format": "json",
    "verbose": false,
    "outputFile": ".cache/cs-validator/last-run.json"
  }
}
```

### Validation Levels Explained

| Level | What it does | Dependencies | Typical time |
|-------|-------------|--------------|--------------|
| 1 | Regex-based syntax: balanced `{}`, `()`, `[]`; namespace/class structure; common patterns | None (pure Rust) | <100ms |
| 2 | Level 1 + `dotnet build --no-restore --nologo` | .NET SDK | 2-4s |
| 3 | Level 2 + `-p:RunAnalyzers=true -p:EnforceCodeStyleInBuild=true` | .NET SDK | 3-5s |
| 4 | Level 3 + `dotnet format --verify-no-changes` | .NET SDK | 4-6s |

### Graceful Degradation

If `dotnet` is not installed and a level ≥2 is requested, the CLI:

1. Prints a clear error: `error: dotnet CLI not found (required for level 2+)`
2. Exits with code 4
3. Does NOT silently fall back to Level 1

This ensures CI pipelines fail loudly rather than producing misleading "pass" results.

### Migration from Shell Scripts

The Rust CLI is a drop-in replacement for `tools/cs-validator.sh`:

| Old (shell) | New (Rust CLI) |
|------------|----------------|
| `./tools/cs-validator.sh --level 2` | `amplihack cs-validate --level 2 .` |
| `./tools/cs-validator.sh --level 4 --verbose` | `amplihack cs-validate --level 4 .` |
| `SKIP_CS_VALIDATION=1` | (omit the command) |

The shell scripts remain for backward compatibility but are deprecated.

---

## Claude Code Integration

The primary use case for this tool is integration with Claude Code's stop hooks.

### Setup (Rust CLI — recommended)

1. **Add to your stop hook** (`.claude/hooks/stop.sh`):

   ```bash
   #!/bin/bash
   # Run C# validation after Claude Code edits
   amplihack cs-validate --level 2 .
   ```

2. **Customize validation level** (optional):
   Edit `~/.amplihack/.claude/hooks/stop.sh` and change the `--level` parameter:

   ```bash
   amplihack cs-validate --level 3 .
   ```

3. **Configure validation settings** (optional):
   Edit `~/.amplihack/.claude/config/cs-validator.json` to customize behavior

### Setup (Legacy shell scripts)

1. **Copy the stop hook example**:

   ```bash
   cp .claude/hooks/stop.sh.example .claude/hooks/stop.sh
   chmod +x .claude/hooks/stop.sh
   ```

2. **Customize validation level** (optional):
   Edit `~/.amplihack/.claude/hooks/stop.sh` and change the `--level` parameter:

   ```bash
   "$VALIDATOR_SCRIPT" --level 2 --verbose
   ```

3. **Configure validation settings** (optional):
   Edit `~/.amplihack/.claude/config/cs-validator.json` to customize behavior

### Usage

The hook runs automatically after Claude Code edits:

1. Claude Code makes changes to C# files
2. You review the changes
3. When you stop/pause, the hook runs automatically
4. If validation fails, you see clear error messages
5. Fix errors and continue

### Customization

**Skip validation temporarily**:

```bash
export SKIP_CS_VALIDATION=1
# Work with Claude Code
# Validation will be skipped
unset SKIP_CS_VALIDATION
```

**Change validation level per session**:
Edit `~/.amplihack/.claude/hooks/stop.sh` before the session:

```bash
# For quick iterations (syntax only)
"$VALIDATOR_SCRIPT" --level 1

# For thorough validation (all checks)
"$VALIDATOR_SCRIPT" --level 4 --verbose
```

### Best Practices

1. **Use level 2 for development**: Fast enough, catches most errors
2. **Use level 4 before PRs**: Ensure code quality
3. **Don't disable permanently**: Keep validation enabled for code quality
4. **Review errors immediately**: Fix issues while context is fresh

---

## Git Hooks Integration

Integrate with standard Git hooks for broader team adoption.

### Pre-commit Hook

Run validation before each commit:

1. **Create pre-commit hook**:

   ```bash
   cat > .git/hooks/pre-commit << 'EOF'
   #!/bin/bash
   # Run C# validation before commit

   set -e

   # Get staged .cs files
   STAGED_CS=$(git diff --cached --name-only --diff-filter=ACMR | grep '\.cs$' || true)

   if [ -z "$STAGED_CS" ]; then
       exit 0
   fi

   echo "Running C# validation on staged files..."

   # Run validator with level 2 (syntax + build)
   if ! amplihack cs-validate --level 2 .; then
       echo ""
       echo "Commit blocked by validation errors"
       echo "Fix the errors above or use: git commit --no-verify"
       exit 1
   fi

   exit 0
   EOF

   chmod +x .git/hooks/pre-commit
   ```

2. **Test the hook**:
   ```bash
   # Make a change with an error
   echo "class Test { // missing closing brace" >> Test.cs
   git add Test.cs
   git commit -m "test"
   # Should fail validation
   ```

### Pre-push Hook

Run validation before pushing to remote:

```bash
cat > .git/hooks/pre-push << 'EOF'
#!/bin/bash
# Run full validation before push

set -e

echo "Running full C# validation before push..."

if ! amplihack cs-validate --level 3 .; then
    echo ""
    echo "Push blocked by validation errors"
    echo "Fix the errors above or use: git push --no-verify"
    exit 1
fi

exit 0
EOF

chmod +x .git/hooks/pre-push
```

### Commit Message Hook

Add validation status to commit messages:

```bash
cat > .git/hooks/prepare-commit-msg << 'EOF'
#!/bin/bash
# Add validation status to commit message

COMMIT_MSG_FILE=$1

# Run quick validation
if ./tools/cs-validator.sh --level 1 > /dev/null 2>&1; then
    echo "" >> "$COMMIT_MSG_FILE"
    echo "✓ C# validation passed" >> "$COMMIT_MSG_FILE"
fi
EOF

chmod +x .git/hooks/prepare-commit-msg
```

### Team-wide Git Hooks

Use tools like [Husky](https://typicode.github.io/husky/) for cross-platform hooks:

1. **Install Husky**:

   ```bash
   npm install --save-dev husky
   npx husky install
   ```

2. **Add pre-commit hook**:

   ```bash
   npx husky add .husky/pre-commit "./tools/cs-validator.sh --level 2"
   ```

3. **Commit hooks to repository**:
   ```bash
   git add .husky
   git commit -m "Add C# validation hooks"
   ```

---

## CI/CD Integration

Integrate validation into your continuous integration pipeline.

### GitHub Actions

```yaml
# .github/workflows/cs-validation.yml
name: C# Validation

on:
  pull_request:
    paths:
      - "**.cs"
      - "**.csproj"
  push:
    branches: [main, develop]

jobs:
  validate:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v3

      - name: Setup .NET
        uses: actions/setup-dotnet@v3
        with:
          dotnet-version: "8.0.x"

      - name: Install amplihack CLI
        run: cargo install amplihack-cli

      - name: Restore dependencies
        run: dotnet restore

      - name: Run C# Validation
        run: amplihack cs-validate --level 3 --format json src/ > validation-results.json

      - name: Upload validation results
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: validation-results
          path: validation-results.json
```

### Azure Pipelines

```yaml
# azure-pipelines.yml
trigger:
  branches:
    include:
      - main
      - develop
  paths:
    include:
      - "**/*.cs"
      - "**/*.csproj"

pool:
  vmImage: "ubuntu-latest"

steps:
  - task: UseDotNet@2
    inputs:
      version: "8.0.x"

  - script: |
      cargo install amplihack-cli
    displayName: "Install amplihack CLI"

  - script: |
      dotnet restore
    displayName: "Restore NuGet packages"

  - script: |
      amplihack cs-validate --level 3 --format json src/ > validation-results.json
    displayName: "Run C# validation"

  - task: PublishBuildArtifacts@1
    condition: always()
    inputs:
      pathToPublish: "validation-results.json"
      artifactName: "validation-results"
```

### Jenkins Pipeline

```groovy
// Jenkinsfile
pipeline {
    agent any

    environment {
        DOTNET_CLI_HOME = '/tmp/dotnet'
    }

    stages {
        stage('Setup') {
            steps {
                sh 'cargo install amplihack-cli'
            }
        }

        stage('Restore') {
            steps {
                sh 'dotnet restore'
            }
        }

        stage('Validate') {
            steps {
                sh 'amplihack cs-validate --level 3 --format json src/ > validation-results.json'
            }
        }
    }

    post {
        always {
            archiveArtifacts artifacts: 'validation-results.json',
                           allowEmptyArchive: true
        }
        failure {
            echo 'C# validation failed!'
        }
    }
}
```

### GitLab CI

```yaml
# .gitlab-ci.yml
cs-validation:
  stage: test
  image: mcr.microsoft.com/dotnet/sdk:8.0

  before_script:
    - curl -sSf https://sh.rustup.rs | sh -s -- -y
    - source "$HOME/.cargo/env"
    - cargo install amplihack-cli
    - dotnet restore

  script:
    - amplihack cs-validate --level 3 --format json src/ > validation-results.json

  artifacts:
    when: always
    paths:
      - validation-results.json
    expire_in: 1 week

  only:
    changes:
      - "**/*.cs"
      - "**/*.csproj"
```

---

## IDE Integration

### Visual Studio Code

Create a task to run validation:

```json
// .vscode/tasks.json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Validate C# (Quick)",
      "type": "shell",
      "command": "${workspaceFolder}/tools/cs-validator.sh --level 2",
      "group": {
        "kind": "test",
        "isDefault": false
      },
      "presentation": {
        "reveal": "always",
        "panel": "new"
      },
      "problemMatcher": []
    },
    {
      "label": "Validate C# (Full)",
      "type": "shell",
      "command": "${workspaceFolder}/tools/cs-validator.sh --level 4 --verbose",
      "group": {
        "kind": "test",
        "isDefault": false
      },
      "presentation": {
        "reveal": "always",
        "panel": "new"
      },
      "problemMatcher": []
    }
  ]
}
```

Add keyboard shortcuts:

```json
// .vscode/keybindings.json
[
  {
    "key": "ctrl+shift+v",
    "command": "workbench.action.tasks.runTask",
    "args": "Validate C# (Quick)"
  }
]
```

### Visual Studio

Create an external tool:

1. Go to **Tools** → **External Tools...**
2. Click **Add**
3. Configure:
   - **Title**: Validate C# (Quick)
   - **Command**: `bash`
   - **Arguments**: `tools/cs-validator.sh --level 2`
   - **Initial directory**: `$(SolutionDir)`
   - Check: **Use Output window**

### Rider

Add a run configuration:

1. **Run** → **Edit Configurations...**
2. Click **+** → **Shell Script**
3. Configure:
   - **Name**: Validate C# (Quick)
   - **Script path**: `tools/cs-validator.sh`
   - **Script options**: `--level 2`
   - **Working directory**: `$ProjectFileDir$`

---

## Custom Workflows

### Watch Mode (Development)

Create a watch script for continuous validation:

```bash
#!/bin/bash
# watch-validate.sh - Watch for changes and validate

while true; do
    inotifywait -e modify -r . --include '\.cs$' 2>/dev/null
    clear
    echo "Change detected, running validation..."
    ./tools/cs-validator.sh --level 2
    echo ""
    echo "Watching for changes... (Ctrl+C to stop)"
done
```

Usage:

```bash
chmod +x watch-validate.sh
./watch-validate.sh
```

### Docker Integration

Run validation in a container:

```dockerfile
# Dockerfile.validator
FROM mcr.microsoft.com/dotnet/sdk:8.0

RUN apt-get update && apt-get install -y \
    python3 \
    jq \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

COPY tools/ /workspace/tools/
COPY .claude/ /workspace/.claude/

RUN chmod +x /workspace/tools/*.sh /workspace/tools/*.py

ENTRYPOINT ["/workspace/tools/cs-validator.sh"]
CMD ["--level", "3"]
```

Build and run:

```bash
docker build -f Dockerfile.validator -t cs-validator .
docker run --rm -v $(pwd):/workspace cs-validator --level 3
```

### Makefile Integration

```makefile
# Makefile
.PHONY: validate validate-quick validate-full

validate: validate-quick

validate-quick:
	@./tools/cs-validator.sh --level 2

validate-full:
	@./tools/cs-validator.sh --level 4 --verbose

validate-ci:
	@./tools/cs-validator.sh --level 3

.PHONY: pre-commit
pre-commit: validate-quick
	@echo "Validation passed, ready to commit"
```

Usage:

```bash
make validate        # Quick validation
make validate-full   # Full validation
make validate-ci     # CI validation
```

---

## Troubleshooting

### Common Integration Issues

#### Issue: Hook not running

**Symptoms**: Stop hook doesn't execute after Claude Code edits

**Solutions**:

1. Check hook is executable: `ls -l .claude/hooks/stop.sh`
2. Verify hook exists: `test -f .claude/hooks/stop.sh && echo "exists"`
3. Check Claude Code configuration

#### Issue: Validation too slow in CI

**Symptoms**: CI builds timeout or take too long

**Solutions**:

1. Use validation level 2 instead of 4
2. Skip test projects in configuration
3. Cache dotnet packages
4. Use faster CI runners

#### Issue: Different results locally vs CI

**Symptoms**: Validation passes locally but fails in CI

**Solutions**:

1. Ensure same .NET SDK version
2. Check git line endings (CRLF vs LF)
3. Verify all dependencies installed in CI
4. Check for environment-specific configuration

#### Issue: False positives in analyzer check

**Symptoms**: Analyzer reports errors that aren't real issues

**Solutions**:

1. Adjust severity threshold to "Error"
2. Configure skip patterns for specific rules
3. Update .editorconfig to match project standards

### Performance Optimization

#### For Large Projects

1. **Skip test projects**:

   ```json
   {
     "skipProjects": ["Tests/**/*.csproj", "**/*.Tests.csproj"]
   }
   ```

2. **Increase timeout**:

   ```json
   {
     "timeoutSeconds": 60
   }
   ```

3. **Use lower validation level**:
   ```bash
   ./tools/cs-validator.sh --level 2
   ```

#### For CI/CD

1. **Cache dependencies**:

   ```yaml
   # GitHub Actions
   - uses: actions/cache@v3
     with:
       path: ~/.nuget/packages
       key: ${{ runner.os }}-nuget-${{ hashFiles('**/*.csproj') }}
   ```

2. **Restore once**:

   ```bash
   dotnet restore
   ./tools/cs-validator.sh --level 3
   ```

3. **Parallel builds**:
   ```bash
   dotnet build -m:4  # Use 4 parallel processes
   ```

---

## Support and Resources

### Getting Help

1. Check [README.md](#) for basic usage
2. Review [ARCHITECTURE.md](#) for design details
3. Search existing GitHub issues
4. Create new issue with:
   - Integration environment (CI, IDE, hooks)
   - Error messages
   - Configuration files
   - Steps to reproduce

### Additional Resources

- [.NET CLI Documentation](https://docs.microsoft.com/en-us/dotnet/core/tools/)
- [Git Hooks Documentation](https://git-scm.com/docs/githooks)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Claude Code Documentation](https://claude.com/claude-code)

### Contributing

To improve integration support:

1. Test new integration scenarios
2. Document your setup
3. Submit PR with examples
4. Update this guide
