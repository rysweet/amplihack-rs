<!-- Ported from upstream amplihack. Rust-specific adaptations applied where applicable. -->

# Profile Management

Amplihack's profile system filters which components get staged during installation, reducing token usage and focusing your environment for specific workflows.

## How It Works

Profiles control **file staging** - which files get staged when you run `amplihack install`. Claude Code sees only the filtered files (no runtime awareness needed).

**Key Principle**: Profile switching happens OUTSIDE Claude Code. To change profiles, you must exit Claude, set a new profile, and restart.

!!! note "Upstream Command References"
    The upstream Python implementation uses `amplihack claude` as an explicit launcher
    command. In amplihack-rs, use `amplihack install` for installation and staging.
    The `amplihack launch` alias is a compatibility feature from the upstream implementation.

## Quick Start

```bash
# Set profile via environment variable
export AMPLIHACK_PROFILE=amplihack://profiles/coding

# Install with profile filtering
amplihack install
# Result: Only 9/32 agents copied (72% reduction)
```

## Built-in Profiles

| Profile           | Agents    | Use Case                       |
| ----------------- | --------- | ------------------------------ |
| **all** (default) | 32 agents | General use, full capabilities |
| **coding**        | 9 agents  | Feature development, bug fixes |
| **research**      | 7 agents  | Code analysis, investigation   |

### Coding Profile

**Included agents** (9):

- architect, builder, reviewer, tester
- api-designer, optimizer
- database, security, cleanup

**Excluded agents** (23):

- knowledge-archaeologist
- All \*-analyst agents (economist, biologist, etc.)
- PM architect
- Specialized workflow agents

### Research Profile

**Included agents** (7):

- architect, analyzer
- knowledge-archaeologist, patterns
- All \*-analyst agents

**Excluded agents**:

- builder, tester (coding-focused)

## Usage

### Set Profile via Environment Variable

```bash
# Built-in profile
export AMPLIHACK_PROFILE=amplihack://profiles/coding

# Local file
export AMPLIHACK_PROFILE=file:///home/user/.amplihack/my-profile.yaml

# GitHub repository
export AMPLIHACK_PROFILE=git+https://github.com/myteam/profiles/blob/main/custom.yaml

# Then install
amplihack install
```

### Profile Priority

1. **AMPLIHACK_PROFILE environment variable** (highest)
2. **No profile set** = "all" profile (copy everything)

### Supported URI Schemes

- `amplihack://profiles/name` - Built-in profiles (~/.amplihack/.claude/profiles/\*.yaml)
- `file:///path/to/profile.yaml` - Local filesystem
- `git+https://github.com/user/repo/blob/ref/path/to/profile.yaml` - GitHub repository

### Workflow

```bash
# 1. Set profile (BEFORE launching Claude)
export AMPLIHACK_PROFILE=amplihack://profiles/coding

# 2. Install with profile filtering
amplihack install  # Stages filtered files globally

# 3. Claude Code sees only filtered components
# (no profile awareness - just sees what files exist)

# 4. To switch profiles: Exit Claude, change profile, restart
exit  # Exit Claude
export AMPLIHACK_PROFILE=amplihack://profiles/research
amplihack install
```

## Creating Custom Profiles

### Example: Minimal Profile

Create `~/.amplihack/.claude/profiles/minimal.yaml`:

```yaml
version: "1.0"
name: "minimal"
description: "Ultra-minimal for quick tasks"

components:
  commands:
    include:
      - "analyze"
      - "fix"

  context:
    include:
      - "PHILOSOPHY.md"

  agents:
    include:
      - "builder"
      - "reviewer"
    exclude:
      - "*" # Exclude all except explicitly included

  skills:
    include: [] # No skills
```

### Use Custom Profile

**Local file:**

```bash
export AMPLIHACK_PROFILE=file://$HOME/.amplihack/profiles/minimal.yaml
amplihack install
```

**From GitHub:**

```bash
# Use profile from your team's repo
export AMPLIHACK_PROFILE=git+https://github.com/myteam/amplihack-profiles/blob/main/minimal.yaml
amplihack install

# Profile is cloned to ~/.amplihack/cache/repos/ and cached for reuse
```

## Profile YAML Schema

```yaml
version: "1.0" # Required
name: "profile-name" # Required
description: "..." # Required

components: # Required
  commands:
    include: [...] # List of command names
    exclude: [...] # Optional exclude patterns
    include_all: false # Or true to include everything

  agents:
    include: [...] # List of agent names (without .md)
    exclude: [...] # Patterns like "*-analyst"
    include_all: false

  context:
    include: [...] # Context file names
    include_all: false

  skills:
    include_categories: [...] # Skill categories
    include: [...] # Individual skills
    include_all: false

metadata: # Optional
  author: "..."
  version: "1.0.0"
  tags: [...]

performance: # Optional
  lazy_load_skills: true
  cache_ttl: 3600
```

## Pattern Matching

Patterns support wildcards:

- `"architect"` matches `architect.md`
- `"*-analyst"` matches `economist-analyst.md`, `biologist-analyst.md`, etc.
- `"ddd:*"` matches `ddd:1-plan.md`, `ddd:2-docs.md`, etc.

## Technical Details

### File Staging Flow

```
User sets: AMPLIHACK_PROFILE=amplihack://profiles/coding
     ↓
amplihack install runs
     ↓
Load profile YAML from .claude/profiles/coding.yaml
     ↓
Create file filter based on include/exclude patterns
     ↓
Copy only files matching profile to .claude/
     ↓
Claude Code launches, sees filtered environment
```

### Module Location

!!! note "Upstream Python Paths"
    The following module paths reference the upstream Python implementation.
    In amplihack-rs, profile management logic is implemented in Rust crates.

- **Profile YAML files**: `~/.amplihack/.claude/profiles/*.yaml`
- **Implementation** (upstream Python):
    - `~/.amplihack/.claude/tools/amplihack/profile_management/staging.py` - File staging logic
    - `~/.amplihack/.claude/tools/amplihack/profile_management/loader.py` - Profile loading
    - `~/.amplihack/.claude/tools/amplihack/profile_management/parser.py` - YAML parsing
    - `~/.amplihack/.claude/tools/amplihack/profile_management/config.py` - Configuration management
- **Integration** (upstream Python): `src/amplihack/__init__.py` (install), `src/amplihack/cli.py` (launch)

### Error Handling

Profile loading uses fail-open design:

- Invalid profile -> Falls back to "all" profile (full installation)
- Missing profile file -> Uses "all" profile
- Parse errors -> Uses "all" profile
- Filter errors -> Includes file (fail-open)

This ensures `amplihack install` never fails due to profile issues.

## Testing

Verify profile filtering works:

```bash
# Install with coding profile
export AMPLIHACK_PROFILE=amplihack://profiles/coding
amplihack install

# Check agent count (should be 9, not 32)
find ~/.claude/agents/amplihack -name "*.md" | wc -l

# Verify specific agents
ls ~/.claude/agents/amplihack/core/architect.md  # Should exist
ls ~/.claude/agents/amplihack/specialized/knowledge-archaeologist.md  # Should NOT exist
```

## Troubleshooting

### Profile not being used

Check environment variable:

```bash
echo $AMPLIHACK_PROFILE
# Should show: amplihack://profiles/coding
```

### All files still copied

- Profile name might be "all" (default)
- Check: `cat ~/.claude/profiles/coding.yaml` exists
- Verify: Profile YAML is valid

### Wrong files copied

- Check profile include/exclude patterns
- Remember: patterns match against file stem (without .md extension)
- Use wildcards carefully: `"*-analyst"` excludes ALL analyst agents

## Related

- `~/.amplihack/.claude/profiles/`: Built-in profile configurations
- [Environment Variables](environment-variables.md): Environment variable reference including AMPLIHACK_PROFILE
- [install Command](install-command.md): Installation command reference
