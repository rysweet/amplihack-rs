#!/usr/bin/env bash
# Skills inherit runtime model selection. Historical/poetic/API references are
# contextual exceptions, not blanket exclusions for whole files. Run from any cwd.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
python3 - "$repo_root" <<'PY'
from pathlib import Path
import re
import sys

repo = Path(sys.argv[1])
model_id = re.compile(
    r"\b(?:claude[- ](?:\d[\w.-]*|opus|sonnet|haiku)|gpt[- ]\d[\w.-]*|"
    r"(?:opus|sonnet|haiku)[ -]\d[\w.-]*|gemini[- ]\d[\w.-]*|"
    r"(?:llama|mistral|deepseek|qwen)[- ]\d[\w.-]*|"
    r"o[134](?:-\w+)?|(?:deepseek|mistral|qwen)-(?:chat|coder|reasoner|large|small)[\w.-]*)\b",
    re.IGNORECASE,
)
family = re.compile(r"\b(?:opus|sonnet|haiku|gemini|llama|mistral|deepseek|qwen)\b", re.I)
execution = re.compile(
    r"(?:\b(?:model|engine|default_model|primary_model|fallback_model|llm_model)\b[\"']?\s*[=:]|--model\b|/model\b|"
    r"\b(?:use|select|choose|recommend\w*|prefer\w*|default|fallback|primary)\b)", re.I,
)
# Exact reviewed lines only. New historical or provider-specific API references
# should be added here with a reason; never exempt a file containing execution guidance.
contextual = {
    ("computer-scientist-analyst/tests/quiz.md",
     "- **Historical Grounding** (0-10): References to GPT-3/4, scaling laws, factuality research"):
        "Historical research rubric, not execution selection",
}


def prohibited(relative, line, exceptions=contextual):
    if (relative, line.strip()) in exceptions:
        return False
    generic_selection = re.search(
        r"(?:\b(?:model|default_model|primary_model|fallback_model|llm_model)\b[\"']?\s*[=:]\s*[\"']?(?:claude|gpt)\b|"
        r"(?:--model\b|/model\b)\s*(?:=\s*)?[\"']?(?:claude|gpt)\b|"
        r"\b(?:use|select|choose|prefer|recommend)\s+(?:the\s+)?(?:claude|gpt)\s+(?:model\s+)?(?:for|as)\b)", line, re.I)
    return bool(model_id.search(line) or generic_selection
                or (family.search(line) and execution.search(line)))


# Regression cases exercise model aliases, versions, prose, and scoped exceptions.
for line in (
    'Use Claude for this skill', 'Recommended model: Claude',
    'default_model = \"GPT\"', 'Use the Claude model for this skill', 'model: GPT', 'model: llama',
    'primary_model: sonnet', 'fallback_model = "haiku"',
    'run --model claude', '/model gpt', 'run --model=claude',
    'model: sonnet', 'model: "haiku"', 'model = "opus"',
    '"model": "sonnet"', "{'model': 'haiku'}",
    'Use Opus for complex tasks', 'Recommended model: Gemini',
    'python run.py --model {opus|sonnet}', '/model sonnet',
    'Use Claude Sonnet 4.5', 'model: claude-opus-4-6',
    'model="gpt-4o"', 'model: o3-mini', 'Prefer DeepSeek-R1',
    'model: qwen-2.5', 'model: mistral-large',
):
    assert prohibited('fixture.md', line), f"Missed execution pin: {line}"
for line in (
    '- **Sonnet**: 14 lines, structured argument',
    '- Haiku: Juxtaposition creates turn',
    '- Examples: GPT, Claude, LLaMA',
    'model: "<configured-model>"', 'model: ConfiguredModel',
    'Use the runtime-configured model',
):
    assert not prohibited('fixture.md', line), f"Rejected neutral/contextual line: {line}"
for (relative, line) in contextual:
    assert not prohibited(relative, line)
    assert prohibited('another-file.md', line), 'Exception leaked across files'
    assert prohibited(relative, line + '; use GPT-4'), 'Exception allowed appended guidance'
# A reviewed provider API example can retain an identifier without excluding its file.
api_line = 'client.messages.create(model="claude-sonnet-4-5", messages=messages)'
api_context = {('api-example.md', api_line): 'Provider API contract illustration'}
assert not prohibited('api-example.md', api_line, api_context)
assert prohibited('api-example.md', 'Use Claude Sonnet 4.5', api_context)

# Static source contracts: no SDK import, compilation, or provider client execution.
# Check each standalone example and mirror, including all invalid-value branches.
for tree in ('amplifier-bundle/skills', 'docs/claude/skills'):
    for name in ('04-basic-agent.cs', '05-tool-integration.cs', '06-simple-workflow.cs'):
        path = repo / tree / 'microsoft-agent-framework/examples' / name
        source = path.read_text()
        assert 'string.IsNullOrWhiteSpace(value)' in source, f'{path}: missing blank configuration rejection'
        assert 'value.Contains("<")'  in source and 'value.Contains(">")'  in source, f'{path}: missing placeholder rejection'
        # Evaluate the restricted validation expression as text, never execute C#.
        condition = re.search(r'if \((string.IsNullOrWhiteSpace\(value\).*?)\)\s*\{', source).group(1)
        expected = 'string.IsNullOrWhiteSpace(value) || value.Contains("<") || value.Contains(">")'
        assert condition == expected, f'{path}: unreviewed configuration validation expression'
        for value, rejected in ((None, True), ('', True), (' \t\n', True),
                                ('<configured-model>', True), (' <primary-model> ', True),
                                ('deployment-name', False), (' user-model ', False)):
            actual = value is None or not value.strip() or '<' in value or '>' in value
            assert actual == rejected, f'{path}: configuration rejection contract failed'
        assert 'return value.Trim();' in source, f'{path}: configured value must be trimmed'
        assert 'throw new InvalidOperationException("Set AGENT_MODEL' in source, f'{path}: error must name setting'
        assert 'model: ConfiguredModel' in source, f'{path}: client must use validated configuration'

failures = []
skills = 0
skill_paths = set()
files = 0
for root in (repo / 'amplifier-bundle/skills', repo / 'docs/claude/skills'):
    if not root.is_dir():
        failures.append(f'Missing skill tree: {root}')
        continue
    # Directory links point to already audited trees or unavailable legacy assets;
    # do not follow them recursively (cycles/duplicate coverage).
    for path in sorted(root.rglob('*')):
        if not path.is_file():
            continue
        try:
            content = path.read_text()
        except UnicodeError as error:
            failures.append(f'{path.relative_to(repo).as_posix()}: cannot decode audit input: {error}')
            continue
        files += 1
        relative = path.relative_to(root).as_posix()
        label = path.relative_to(repo).as_posix()
        if path.name == 'SKILL.md' and root.parent.name == 'amplifier-bundle':
            skills += 1
            skill_paths.add(path.parent.relative_to(root).as_posix())
            parts = content.split('---', 2)
            if len(parts) != 3 or parts[0].strip():
                failures.append(f'{label}: missing frontmatter')
            elif re.search(r'^\s*model\s*:', parts[1], re.MULTILINE | re.I):
                failures.append(f'{label}: execution model override in frontmatter')
        for number, line in enumerate(content.splitlines(), 1):
            if prohibited(relative, line):
                failures.append(f'{label}:{number}: concrete execution model reference: {line.strip()}')
# Check identities as well as count: replacing a reviewed skill must fail.
audit = repo / 'amplifier-bundle/recipes/tests/skill-model-neutrality-audit.md'
reviewed = set(re.findall(r'^\| ([^|]+) \| Pass \| (?:Yes|No) \|$', audit.read_text(), re.MULTILINE))
if len(reviewed) != 130:
    failures.append('Audit must enumerate exactly 130 distinct reviewed skills.')
if skill_paths != reviewed:
    failures.append('Reviewed skill inventory changed: missing=' + ', '.join(sorted(reviewed - skill_paths))
                    + '; added=' + ', '.join(sorted(skill_paths - reviewed)))
if skills != 130:
    failures.append(f'Expected 130 skills; scanned {skills}. Review inventory before updating this guard.')
if failures:
    print('\n'.join(failures), file=sys.stderr)
    sys.exit(1)
print(f'PASS: {skills} skills; {files} text files across skills and documentation mirrors; detector regression cases passed.')
PY
