//! Condition expression evaluator for recipe step conditions.
//!
//! Supports the expression grammar used in recipe YAML `condition:` fields:
//!
//! - String literals: `'foo'`, `"foo"`
//! - Integer literals: `1`, `42`
//! - Identifiers (context variables): `task_type`, `round_1_result`
//! - Dot access: `obj.field.subfield`
//! - Boolean operators: `and`, `or`, `not`
//! - Comparison: `==`, `!=`, `>=`, `<=`, `>`, `<`
//! - Containment: `'x' in var`, `'x' not in var`
//! - List literals: `['a', 'b', 'c']`
//! - Parenthesized groups: `(expr)`
//! - Function calls: `int(x)`
//! - Truthiness: bare identifiers are truthy if non-empty and not `"false"`

use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Evaluate a condition expression against a context of string variables.
///
/// Returns `Ok(true)` / `Ok(false)` for valid expressions, or `Err` with a
/// parse/eval error message.
pub fn evaluate_condition(
    expr: &str,
    context: &HashMap<String, String>,
) -> Result<bool, ConditionError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(true); // empty condition → always run
    }
    let tokens = tokenize(trimmed)?;
    if tokens.is_empty() {
        return Ok(true);
    }
    let mut pos = 0;
    let value = parse_or(&tokens, &mut pos, context)?;
    if pos < tokens.len() {
        return Err(ConditionError::Parse(format!(
            "unexpected token after expression: {:?}",
            tokens[pos]
        )));
    }
    Ok(value.is_truthy())
}

/// Validate that a condition expression can be parsed (without evaluating).
///
/// Uses a permissive evaluation mode where missing variables return empty
/// strings and function call errors are ignored — we only check that the
/// token stream and grammar are valid.
pub fn validate_condition(expr: &str) -> Result<(), ConditionError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let tokens = tokenize(trimmed)?;
    if tokens.is_empty() {
        return Ok(());
    }
    let empty: HashMap<String, String> = HashMap::new();
    let mut pos = 0;
    let _ = parse_or_validating(&tokens, &mut pos, &empty)?;
    if pos < tokens.len() {
        return Err(ConditionError::Parse(format!(
            "unexpected token after expression: {:?}",
            tokens[pos]
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ConditionError {
    Parse(String),
    Eval(String),
}

impl fmt::Display for ConditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConditionError::Parse(msg) => write!(f, "Parse error: {msg}"),
            ConditionError::Eval(msg) => write!(f, "Eval error: {msg}"),
        }
    }
}

impl std::error::Error for ConditionError {}

// ---------------------------------------------------------------------------
// Value type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<Value>),
    /// Represents an undefined/missing variable (falsy).
    None,
}

impl Value {
    fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty() && s != "false" && s != "False" && s != "0",
            Value::Int(n) => *n != 0,
            Value::List(items) => !items.is_empty(),
            Value::None => false,
        }
    }

    fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::List(_) => "<list>".to_string(),
            Value::None => String::new(),
        }
    }

    fn contains(&self, needle: &Value) -> bool {
        match self {
            Value::Str(haystack) => {
                let needle_str = needle.as_str();
                haystack.contains(&needle_str)
            }
            Value::List(items) => items.iter().any(|item| item.as_str() == needle.as_str()),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Int(i64),
    Eq,       // ==
    Ne,       // !=
    Ge,       // >=
    Le,       // <=
    Gt,       // >
    Lt,       // <
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Dot,      // .
    And,      // and
    Or,       // or
    Not,      // not
    In,       // in
    True,
    False,
}

fn tokenize(input: &str) -> Result<Vec<Token>, ConditionError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Two-character operators
        if i + 1 < len {
            let two = &input[i..i + 2];
            match two {
                "==" => {
                    tokens.push(Token::Eq);
                    i += 2;
                    continue;
                }
                "!=" => {
                    tokens.push(Token::Ne);
                    i += 2;
                    continue;
                }
                ">=" => {
                    tokens.push(Token::Ge);
                    i += 2;
                    continue;
                }
                "<=" => {
                    tokens.push(Token::Le);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        match ch {
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
                continue;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
                continue;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
                continue;
            }
            '[' => {
                tokens.push(Token::LBracket);
                i += 1;
                continue;
            }
            ']' => {
                tokens.push(Token::RBracket);
                i += 1;
                continue;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
                continue;
            }
            '.' => {
                tokens.push(Token::Dot);
                i += 1;
                continue;
            }
            _ => {}
        }

        // String literals
        if ch == '\'' || ch == '"' {
            let quote = ch;
            i += 1;
            let start = i;
            while i < len && chars[i] != quote {
                i += 1;
            }
            if i >= len {
                return Err(ConditionError::Parse(format!(
                    "unterminated string literal starting at position {start}"
                )));
            }
            let s: String = chars[start..i].iter().collect();
            tokens.push(Token::Str(s));
            i += 1;
            continue;
        }

        // Numbers
        if ch.is_ascii_digit() {
            let start = i;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            let n: i64 = num_str
                .parse()
                .map_err(|_| ConditionError::Parse(format!("invalid integer: {num_str}")))?;
            tokens.push(Token::Int(n));
            continue;
        }

        // Identifiers and keywords
        if ch.is_ascii_alphanumeric() || ch == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let tok = match word.as_str() {
                "and" => Token::And,
                "or" => Token::Or,
                "not" => Token::Not,
                "in" => Token::In,
                "true" | "True" => Token::True,
                "false" | "False" => Token::False,
                _ => Token::Ident(word),
            };
            tokens.push(tok);
            continue;
        }

        // Special: &amp; in YAML gets decoded to & — handle & in identifiers
        // like 'Q&A' which appear as string literals, not bare identifiers.
        return Err(ConditionError::Parse(format!(
            "unexpected character: '{ch}' at position {i}"
        )));
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Recursive-descent parser
//
// Grammar (precedence low → high):
//   or_expr    = and_expr ("or" and_expr)*
//   and_expr   = not_expr ("and" not_expr)*
//   not_expr   = "not" not_expr | cmp_expr
//   cmp_expr   = primary (("==" | "!=" | ">=" | "<=" | ">" | "<" |
//                           "in" | "not" "in") primary)*
//   primary    = "(" or_expr ")"
//              | "[" list_items "]"
//              | "true" | "false"
//              | INT
//              | STRING
//              | ident ("." ident)* [ "(" args ")" ]
// ---------------------------------------------------------------------------

/// Validation-mode wrapper: function eval errors return defaults.
fn parse_or_validating(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
) -> Result<Value, ConditionError> {
    parse_or_impl(tokens, pos, ctx, true)
}

fn parse_or(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
) -> Result<Value, ConditionError> {
    parse_or_impl(tokens, pos, ctx, false)
}

fn parse_or_impl(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
    lenient: bool,
) -> Result<Value, ConditionError> {
    let mut left = parse_and(tokens, pos, ctx, lenient)?;
    while *pos < tokens.len() && tokens[*pos] == Token::Or {
        *pos += 1;
        let right = parse_and(tokens, pos, ctx, lenient)?;
        left = Value::Bool(left.is_truthy() || right.is_truthy());
    }
    Ok(left)
}

fn parse_and(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
    lenient: bool,
) -> Result<Value, ConditionError> {
    let mut left = parse_not(tokens, pos, ctx, lenient)?;
    while *pos < tokens.len() && tokens[*pos] == Token::And {
        *pos += 1;
        let right = parse_not(tokens, pos, ctx, lenient)?;
        left = Value::Bool(left.is_truthy() && right.is_truthy());
    }
    Ok(left)
}

fn parse_not(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
    lenient: bool,
) -> Result<Value, ConditionError> {
    if *pos < tokens.len() && tokens[*pos] == Token::Not {
        // Peek ahead: "not in" is a binary operator, not unary "not".
        if *pos + 1 < tokens.len() && tokens[*pos + 1] == Token::In {
            return parse_cmp(tokens, pos, ctx, lenient);
        }
        *pos += 1;
        let val = parse_not(tokens, pos, ctx, lenient)?;
        return Ok(Value::Bool(!val.is_truthy()));
    }
    parse_cmp(tokens, pos, ctx, lenient)
}

fn parse_cmp(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
    lenient: bool,
) -> Result<Value, ConditionError> {
    let mut left = parse_primary(tokens, pos, ctx, lenient)?;

    loop {
        if *pos >= tokens.len() {
            break;
        }
        match &tokens[*pos] {
            Token::Eq => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(left.as_str() == right.as_str());
            }
            Token::Ne => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(left.as_str() != right.as_str());
            }
            Token::Ge => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(cmp_numeric_or_str(&left, &right, |a, b| a >= b));
            }
            Token::Le => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(cmp_numeric_or_str(&left, &right, |a, b| a <= b));
            }
            Token::Gt => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(cmp_numeric_or_str(&left, &right, |a, b| a > b));
            }
            Token::Lt => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(cmp_numeric_or_str(&left, &right, |a, b| a < b));
            }
            Token::In => {
                *pos += 1;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(right.contains(&left));
            }
            Token::Not if *pos + 1 < tokens.len() && tokens[*pos + 1] == Token::In => {
                *pos += 2;
                let right = parse_primary(tokens, pos, ctx, lenient)?;
                left = Value::Bool(!right.contains(&left));
            }
            _ => break,
        }
    }

    Ok(left)
}

fn parse_primary(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &HashMap<String, String>,
    lenient: bool,
) -> Result<Value, ConditionError> {
    if *pos >= tokens.len() {
        return Err(ConditionError::Parse(
            "unexpected end of expression".to_string(),
        ));
    }

    match &tokens[*pos] {
        Token::LParen => {
            *pos += 1;
            let val = parse_or_impl(tokens, pos, ctx, lenient)?;
            if *pos >= tokens.len() || tokens[*pos] != Token::RParen {
                return Err(ConditionError::Parse("expected ')'".to_string()));
            }
            *pos += 1;
            Ok(val)
        }
        Token::LBracket => {
            *pos += 1;
            let mut items = Vec::new();
            if *pos < tokens.len() && tokens[*pos] != Token::RBracket {
                items.push(parse_or_impl(tokens, pos, ctx, lenient)?);
                while *pos < tokens.len() && tokens[*pos] == Token::Comma {
                    *pos += 1;
                    if *pos < tokens.len() && tokens[*pos] == Token::RBracket {
                        break; // trailing comma
                    }
                    items.push(parse_or_impl(tokens, pos, ctx, lenient)?);
                }
            }
            if *pos >= tokens.len() || tokens[*pos] != Token::RBracket {
                return Err(ConditionError::Parse("expected ']'".to_string()));
            }
            *pos += 1;
            Ok(Value::List(items))
        }
        Token::Str(s) => {
            let val = Value::Str(s.clone());
            *pos += 1;
            Ok(val)
        }
        Token::Int(n) => {
            let val = Value::Int(*n);
            *pos += 1;
            Ok(val)
        }
        Token::True => {
            *pos += 1;
            Ok(Value::Bool(true))
        }
        Token::False => {
            *pos += 1;
            Ok(Value::Bool(false))
        }
        Token::Ident(name) => {
            let name = name.clone();
            *pos += 1;

            // Dot-access: ident.field.subfield
            let mut lookup_key = name.clone();
            while *pos < tokens.len() && tokens[*pos] == Token::Dot {
                *pos += 1;
                if *pos >= tokens.len() {
                    return Err(ConditionError::Parse(
                        "expected identifier after '.'".to_string(),
                    ));
                }
                if let Token::Ident(field) = &tokens[*pos] {
                    lookup_key = format!("{lookup_key}.{field}");
                    *pos += 1;
                } else {
                    return Err(ConditionError::Parse(format!(
                        "expected identifier after '.', got {:?}",
                        tokens[*pos]
                    )));
                }
            }

            // Function call: int(x)
            if *pos < tokens.len() && tokens[*pos] == Token::LParen {
                *pos += 1;
                let arg = parse_or_impl(tokens, pos, ctx, lenient)?;
                if *pos >= tokens.len() || tokens[*pos] != Token::RParen {
                    return Err(ConditionError::Parse(format!(
                        "expected ')' after function argument for '{lookup_key}'"
                    )));
                }
                *pos += 1;
                return match eval_function(&lookup_key, &arg) {
                    Ok(v) => Ok(v),
                    Err(_) if lenient => Ok(Value::Int(0)),
                    Err(e) => Err(e),
                };
            }

            // Variable lookup
            if let Some(val) = ctx.get(&lookup_key) {
                Ok(Value::Str(val.clone()))
            } else {
                Ok(Value::None)
            }
        }
        other => Err(ConditionError::Parse(format!(
            "unexpected token: {other:?}"
        ))),
    }
}

/// Compare two values numerically if both parse as i64, otherwise by string length.
fn cmp_numeric_or_str(left: &Value, right: &Value, op: fn(i64, i64) -> bool) -> bool {
    let l_int = match left {
        Value::Int(n) => Some(*n),
        _ => left.as_str().trim().parse::<i64>().ok(),
    };
    let r_int = match right {
        Value::Int(n) => Some(*n),
        _ => right.as_str().trim().parse::<i64>().ok(),
    };
    if let (Some(l), Some(r)) = (l_int, r_int) {
        return op(l, r);
    }
    // Fall back to lexicographic comparison
    op(left.as_str().len() as i64, right.as_str().len() as i64)
}

fn eval_function(name: &str, arg: &Value) -> Result<Value, ConditionError> {
    match name {
        "int" => {
            let s = arg.as_str();
            let n: i64 = s.trim().parse().map_err(|_| {
                ConditionError::Eval(format!("int() cannot convert '{s}' to integer"))
            })?;
            Ok(Value::Int(n))
        }
        other => Err(ConditionError::Eval(format!("unknown function: '{other}'"))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // -- Truthiness --

    #[test]
    fn empty_condition_is_true() {
        assert!(evaluate_condition("", &HashMap::new()).unwrap());
        assert!(evaluate_condition("   ", &HashMap::new()).unwrap());
    }

    #[test]
    fn truthy_variable() {
        let c = ctx(&[("round_1_result", "some output")]);
        assert!(evaluate_condition("round_1_result", &c).unwrap());
    }

    #[test]
    fn falsy_missing_variable() {
        assert!(!evaluate_condition("missing_var", &HashMap::new()).unwrap());
    }

    #[test]
    fn falsy_empty_variable() {
        let c = ctx(&[("empty", "")]);
        assert!(!evaluate_condition("empty", &c).unwrap());
    }

    // -- String equality --

    #[test]
    fn string_equality() {
        let c = ctx(&[("task_type", "Development")]);
        assert!(evaluate_condition("task_type == 'Development'", &c).unwrap());
        assert!(!evaluate_condition("task_type == 'Q&A'", &c).unwrap());
    }

    #[test]
    fn string_inequality() {
        let c = ctx(&[("convergence_status", "NOT_CONVERGED")]);
        assert!(evaluate_condition("convergence_status != 'CONVERGED'", &c).unwrap());
    }

    // -- Integer comparison --

    #[test]
    fn int_equality() {
        let c = ctx(&[("workstream_count", "1")]);
        assert!(evaluate_condition("workstream_count == 1", &c).unwrap());
        assert!(evaluate_condition("workstream_count == '1'", &c).unwrap());
    }

    #[test]
    fn int_comparison_operators() {
        let c = ctx(&[("n", "5")]);
        assert!(evaluate_condition("int(n) >= 4", &c).unwrap());
        assert!(evaluate_condition("int(n) >= 5", &c).unwrap());
        assert!(!evaluate_condition("int(n) >= 6", &c).unwrap());
        assert!(evaluate_condition("int(n) > 4", &c).unwrap());
        assert!(!evaluate_condition("int(n) > 5", &c).unwrap());
        assert!(evaluate_condition("int(n) <= 6", &c).unwrap());
        assert!(evaluate_condition("int(n) < 6", &c).unwrap());
    }

    // -- Containment (in / not in) --

    #[test]
    fn string_in_variable() {
        let c = ctx(&[("task_type", "Development")]);
        assert!(evaluate_condition("'Development' in task_type", &c).unwrap());
        assert!(!evaluate_condition("'Q&A' in task_type", &c).unwrap());
    }

    #[test]
    fn string_not_in_variable() {
        let c = ctx(&[("task_type", "Development")]);
        assert!(evaluate_condition("'Q&A' not in task_type", &c).unwrap());
        assert!(!evaluate_condition("'Development' not in task_type", &c).unwrap());
    }

    // -- List literals (the LBracket fix!) --

    #[test]
    fn in_list_literal() {
        let c = ctx(&[("task_type", "Development")]);
        assert!(evaluate_condition("task_type in ['Development', 'Investigation']", &c,).unwrap());
        assert!(!evaluate_condition("task_type in ['Q&A', 'Operations']", &c).unwrap());
    }

    #[test]
    fn not_in_list_literal() {
        let c = ctx(&[("task_type", "Development")]);
        assert!(evaluate_condition("task_type not in ['Q&A', 'Operations']", &c,).unwrap());
        assert!(
            !evaluate_condition("task_type not in ['Development', 'Investigation']", &c,).unwrap()
        );
    }

    #[test]
    fn empty_list() {
        let c = ctx(&[("x", "a")]);
        assert!(!evaluate_condition("x in []", &c).unwrap());
    }

    #[test]
    fn list_is_truthy() {
        let c = HashMap::new();
        assert!(evaluate_condition("['a']", &c).unwrap());
        assert!(!evaluate_condition("[]", &c).unwrap());
    }

    // -- Boolean operators --

    #[test]
    fn and_operator() {
        let c = ctx(&[("a", "yes"), ("b", "yes")]);
        assert!(evaluate_condition("a and b", &c).unwrap());
        let c2 = ctx(&[("a", "yes")]);
        assert!(!evaluate_condition("a and b", &c2).unwrap());
    }

    #[test]
    fn or_operator() {
        let c = ctx(&[("a", "yes")]);
        assert!(evaluate_condition("a or b", &c).unwrap());
        assert!(!evaluate_condition("missing1 or missing2", &c).unwrap());
    }

    #[test]
    fn not_operator() {
        let c = ctx(&[("a", "yes")]);
        assert!(!evaluate_condition("not a", &c).unwrap());
        assert!(evaluate_condition("not missing", &c).unwrap());
    }

    // -- Parentheses --

    #[test]
    fn parenthesized_expression() {
        let c = ctx(&[("a", "yes"), ("b", "")]);
        assert!(evaluate_condition("(a or b) and a", &c).unwrap());
        assert!(!evaluate_condition("(a and b)", &c).unwrap());
    }

    // -- Dot access --

    #[test]
    fn dot_access() {
        let c = ctx(&[("obj.field", "value")]);
        assert!(evaluate_condition("obj.field == 'value'", &c).unwrap());
    }

    // -- Function calls --

    #[test]
    fn int_function() {
        let c = ctx(&[("num_versions", "4")]);
        assert!(evaluate_condition("int(num_versions) >= 4", &c).unwrap());
        assert!(!evaluate_condition("int(num_versions) >= 5", &c).unwrap());
        assert!(evaluate_condition("int(num_versions) == 4", &c).unwrap());
    }

    // -- Complex real-world conditions from smart-orchestrator.yaml --

    #[test]
    fn smart_orchestrator_dev_single() {
        let c = ctx(&[
            ("task_type", "Development"),
            ("workstream_count", "1"),
            ("force_single_workstream", "false"),
        ]);
        assert!(evaluate_condition(
            "'Development' in task_type and ((workstream_count == 1 or workstream_count == '1' or workstream_count == '') or force_single_workstream == 'true')",
            &c,
        ).unwrap());
    }

    #[test]
    fn smart_orchestrator_qa_not_in() {
        let c = ctx(&[
            ("task_type", "Development"),
            ("round_1_result", "some result"),
        ]);
        assert!(
            evaluate_condition(
                "'Q&A' not in task_type and 'Operations' not in task_type and round_1_result",
                &c,
            )
            .unwrap()
        );
    }

    #[test]
    fn code_atlas_simple_equality() {
        let c = ctx(&[("bug_hunt", "true")]);
        assert!(evaluate_condition("bug_hunt == 'true'", &c).unwrap());
    }

    #[test]
    fn auto_workflow_string_contains() {
        let c = ctx(&[("iteration_1", "CONTINUE: more work needed")]);
        assert!(evaluate_condition("'CONTINUE' in iteration_1", &c).unwrap());
    }

    #[test]
    fn quality_audit_not_in_string() {
        let c = ctx(&[("recurse_decision", "STOP: quality is good")]);
        assert!(evaluate_condition("'CONTINUE:' not in recurse_decision", &c).unwrap());
    }

    // -- Validate function --

    #[test]
    fn validate_good_condition() {
        assert!(validate_condition("'Development' in task_type").is_ok());
        assert!(validate_condition("x not in ['a', 'b']").is_ok());
        assert!(validate_condition("int(x) >= 4").is_ok());
        assert!(validate_condition("").is_ok());
    }

    #[test]
    fn validate_bad_condition() {
        assert!(validate_condition("== ==").is_err());
        assert!(validate_condition("'unterminated").is_err());
    }

    // -- Regression: LBracket must not cause parse failure (issue #212) --

    #[test]
    fn lbracket_regression_issue_212() {
        let c = ctx(&[("task_type", "Development")]);
        let result = evaluate_condition("task_type not in ['Q&A', 'Operations', 'Hybrid']", &c);
        assert!(result.is_ok(), "LBracket must be handled: {result:?}");
        assert!(result.unwrap());
    }

    #[test]
    fn lbracket_in_list_regression() {
        let c = ctx(&[("status", "running")]);
        let result = evaluate_condition("status in ['running', 'pending']", &c);
        assert!(result.is_ok(), "LBracket must be handled: {result:?}");
        assert!(result.unwrap());
    }

    // -- Edge cases --

    #[test]
    fn boolean_literals() {
        assert!(evaluate_condition("true", &HashMap::new()).unwrap());
        assert!(!evaluate_condition("false", &HashMap::new()).unwrap());
    }

    #[test]
    fn trailing_comma_in_list() {
        let c = ctx(&[("x", "a")]);
        assert!(evaluate_condition("x in ['a', 'b',]", &c).unwrap());
    }

    #[test]
    fn nested_parentheses_and_list() {
        let c = ctx(&[("t", "Dev"), ("r", "ok")]);
        assert!(evaluate_condition("(t in ['Dev', 'Inv']) and r", &c).unwrap());
    }

    #[test]
    fn double_quoted_strings() {
        let c = ctx(&[("x", "hello")]);
        assert!(evaluate_condition(r#"x == "hello""#, &c).unwrap());
    }

    #[test]
    fn false_string_is_falsy() {
        let c = ctx(&[("flag", "false")]);
        assert!(!evaluate_condition("flag", &c).unwrap());
    }

    #[test]
    fn complex_condition_with_dot_and_equality() {
        let c = ctx(&[("initial_requirements.requires_debate", "true")]);
        assert!(evaluate_condition("initial_requirements.requires_debate == 'true'", &c).unwrap());
    }
}
