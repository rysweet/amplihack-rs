# Reviewing a Pull Request

Read this when the code under review is a pull request, and when you post the findings to it. The review dimensions, the code smells and the output format are in `SKILL.md`. Worked reviews are in `examples.md`.

## Steps

### 1. Understand the Changes

1. Read the pull request description and diff to understand its intent.
2. Identify the affected modules and their scope.
3. Note the dependencies changed or added.
4. Understand the problem being solved.

### 2. Check Philosophy Alignment

Simplicity:

- Is every line necessary?
- Are there needless abstractions?
- Could this be done more simply?
- Is there future-proofing or speculation?
- Are there duplicate or similar functions?
- Could the conditional logic be simpler?

Module structure:

- Does the change respect module boundaries?
- Are public contracts clear and documented?
- Are internal utilities isolated?
- Does the module have one clear responsibility?
- Are there circular dependencies?

No shortcuts:

- Are there TODOs or `NotImplementedError` calls?
- Are mock or test data exposed in production code?
- Is error handling explicit and visible?
- Does every function work?
- Is there dead code or unused variables?

### 3. Identify Over-Engineering

- Over-abstraction: base classes, protocols or factories with no clear benefit.
- Generic frameworks built for hypothetical needs.
- Premature optimisation of paths that are not critical.
- Configuration complexity: a 50-line config where a 5-line default would work.
- Future-proofing: "we might need this someday" code.
- More layers of indirection than needed.
- Long parameter lists (see "Common Code Smells" in `SKILL.md`).

### 4. Check Module Structure

For new modules or changed modules:

- Single responsibility: what is the one thing this module does?
- Clear public interface: what is exported, and why?
- Internal isolation: are utilities kept inside the module?
- Dependencies: what does it depend on, and is that written down?
- Tests: does the specification define the test requirements?
- Examples: is the usage clear?
- Regeneratable: could it be rebuilt from its specification?

### 5. Check Test Coverage

- Tests check the behaviour of the public interface.
- Tests check accepted inputs, return values and errors against the documented contract.
- Edge cases are covered: null, empty and boundary values.
- Error paths are tested: exceptions are raised when expected.
- Module connections are tested.
- Coverage is 85 per cent or more for changed code.

### 6. Check Refactoring and Breaking Changes

For a refactoring:

- Tests still pass, behaviour is unchanged, and the public interface does not change.
- Performance is not worse, and error handling is kept.
- Complexity goes down; dead code and unused imports are removed.

For a breaking change:

- It is marked as breaking.
- A deprecation path or a migration guide is given.
- The affected modules are named.
- The version change follows the repository's versioning policy.

### 7. Give Constructive Feedback

1. Be specific: give `file:line` references.
2. Explain why: say which principle is broken.
3. Suggest how: give a concrete example.
4. Be respectful: comment on the code, not the person.
5. Say what is done well.

When the author has dealt with the findings, review the pull request again at its new head.

## Feedback Template

Use this form for each comment:

```markdown
**FILE**: path/to/file.py (lines X-Y)

**ISSUE**: [Principle broken: Simplicity/Modularity/Zero-BS/Tests/Docs]

**WHAT**: [What is in the code]

**WHY IT'S PROBLEMATIC**: [How it breaks the principle]

**SUGGESTION**: [Concrete code example or approach]

**REFERENCE**: [Link to the principle or example]
```

## Reading and Posting with `gh`

Post feedback on GitHub when the review request calls for it.

```bash
# Read the pull request and its diff
gh pr view <PR-NUMBER>
gh pr diff <PR-NUMBER>

# Post a comment
gh pr comment <PR-NUMBER> --body "Review comment here"
```

Workflow:

1. Fetch the pull request details: number, branch and changed files.
2. Review the changes as above.
3. Compile specific, actionable comments with `file:line` references.
4. Post the review with all the comments.
5. Post an overall assessment with the verdict.

## Common Over-Engineering Patterns

### Configuration Complexity

```python
# OVER-ENGINEERED: 50-line config class
class ConfigManager:
    def __init__(self, env_file, schema_file, validators):
        self.config = load_yaml(env_file)
        self.schema = load_json(schema_file)
        self.validators = validators
        # 40 more lines...

# SIMPLE: 5 lines
config = yaml.safe_load(open('.env.yaml'))
```

### A Factory When None Is Needed

```python
# OVER-ENGINEERED: factory for a single implementation
class ValidationFactory:
    def create_validator(self, type):
        if type == "email":
            return EmailValidator()
        # ... more types

# SIMPLE: a direct function
def validate_email(email):
    return "@" in email and "." in email
```

### A Generic Base Class for One Use

```python
# OVER-ENGINEERED: base class never subclassed twice
class BaseRepository(ABC):
    @abstractmethod
    def find(self, id): pass
    # ... 20 abstract methods

class UserRepository(BaseRepository):
    # Forced to implement all abstract methods,
    # but only uses 3 of them

# SIMPLE: a direct class
class UserRepository:
    def find(self, id):
        return self.db.query(User).get(id)
```

### Premature Optimisation

```python
# OVER-ENGINEERED: complex caching nobody needs
cache = LRUCache(maxsize=1000)
stats = CacheStats()
lock = threading.Lock()
# ... complex logic

# SIMPLE: none. Profile first, optimise if needed
result = function(args)
```

## Key Questions

1. Can this be simpler? If yes, why is it not?
2. Is this needed now, or is it future-proofing?
3. What is the one thing this does? If it does many things, split it.
4. Who will use this? Is the interface clear for them?
5. What can go wrong? Are errors handled explicitly?
6. Is this testable? Can the contract be checked?
7. Will this need to change? Is it flexible without over-engineering?
8. Could this be deleted rather than refactored?
9. Does this follow our patterns, or is it unique?
10. Am I confident this works, or is it speculative?
