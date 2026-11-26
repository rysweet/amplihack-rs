# Writing the Tests

Use these patterns to write the tests suggested in `SKILL.md`. The Python examples apply to other languages and test runners too.

## Arrange, Act, Assert

Give each test three parts:

```python
def test_strip_removes_surrounding_whitespace():
    # Arrange: set up the input and expected result
    text = "  hello  "
    expected = "hello"

    # Act: run the code under test
    result = text.strip()

    # Assert: check the result
    assert result == expected
```

## Test Patterns

### Unit Tests

Test one function in isolation.

```python
from mymodule import calculate_total

def test_calculate_total_with_valid_items():
    items = [{"price": 10.00, "quantity": 2}, {"price": 5.00, "quantity": 3}]
    assert calculate_total(items) == 35.00
```

### Integration Tests

Test several components working together.

```python
def test_user_registration_flow():
    # Arrange: a test database
    db = setup_test_database()
    api = UserAPI(db)

    # Act: register a user
    response = api.register({"email": "test@example.com", "password": "SecurePass123!"})

    # Assert: the user exists and the welcome email was sent
    assert response.status == 201
    assert db.users.find_by_email("test@example.com") is not None
    assert email_was_sent("test@example.com", "Welcome")
```

### Mocking and Stubbing

Isolate the code from external dependencies.

```python
from unittest.mock import Mock, patch

def test_send_notification_calls_email_service():
    mock_email = Mock()

    with patch("mymodule.email_service", mock_email):
        send_notification("user@example.com", "Hello")

    mock_email.send.assert_called_once_with(
        to="user@example.com", subject="Notification", body="Hello"
    )
```

### Parameterised Tests

Test many inputs with one test.

```python
import pytest

@pytest.mark.parametrize("value,expected", [
    (0, "zero"),
    (1, "one"),
    (10, "many"),
    (-5, "negative"),
])
def test_number_to_word(value, expected):
    assert number_to_word(value) == expected
```

For tests that generate their inputs, use the `property-based-testing` skill.

## Test Organisation

### Directory Structure

```
project/
├── src/
│   └── mymodule/
│       ├── calculator.py
│       └── user.py
└── tests/
    ├── unit/
    │   ├── test_calculator.py
    │   └── test_user.py
    ├── integration/
    │   └── test_user_flow.py
    └── conftest.py  # shared fixtures
```

### Naming Conventions

- Test files: `test_*.py` or `*_test.py`
- Test functions: `test_<function>_<condition>_<expected>`
- Test classes: `Test<ClassName>`

Good names:

- `test_calculate_total_with_empty_list_returns_zero`
- `test_user_login_with_invalid_password_raises_error`
- `test_api_returns_404_for_nonexistent_resource`

Bad names:

- `test_calc` (too vague)
- `test_1` (no meaning)
- `test_it_works` (what works?)

## Testing Checklist

For each function or class, test:

Functionality

- [ ] The normal path
- [ ] Return values
- [ ] Side effects

Edge cases

- [ ] Empty inputs ([], "", 0, None)
- [ ] Boundary values (min, max, -1)
- [ ] Large inputs
- [ ] Special characters and values

Error handling

- [ ] Invalid inputs
- [ ] Null, undefined or None
- [ ] Wrong types
- [ ] Constraint violations
- [ ] External failures (an API that is down, for example)

Integration

- [ ] Calls to other functions and modules
- [ ] Database interactions
- [ ] File operations
- [ ] Network requests

## Test Quality Standards

Good tests are:

- Fast: they run in milliseconds, do no needless I/O and mock external dependencies.
- Independent: they share no state, run in any order and depend on no other test.
- Repeatable: they give the same result every time and are never flaky.
- Self-validating: they pass or fail clearly, need no manual inspection and give a clear failure message.
- Timely: they are written before or with the code, and updated when the code changes.

## Anti-Patterns to Avoid

Testing implementation details:

```python
# Bad: tests internal state
def test_cache_uses_dict():
    cache = Cache()
    assert isinstance(cache._storage, dict)

# Good: tests behaviour
def test_cache_stores_and_retrieves_value():
    cache = Cache()
    cache.set("key", "value")
    assert cache.get("key") == "value"
```

Overly complex tests:

```python
# Bad: hard to understand
def test_complex():
    data = [process(x) for x in range(100) if x % 2]
    result = calculate(data)
    assert result == sum(data) / len(data)

# Good: clear and simple
def test_calculate_returns_average():
    assert calculate([2, 4, 6]) == 4.0
```

Testing only the normal path:

```python
# Incomplete: only the normal path
def test_divide():
    assert divide(10, 2) == 5

# Complete: the edge cases too
def test_divide_with_valid_numbers():
    assert divide(10, 2) == 5

def test_divide_by_zero_raises_error():
    with pytest.raises(ZeroDivisionError):
        divide(10, 0)

def test_divide_with_negative_numbers():
    assert divide(-10, 2) == -5
```
