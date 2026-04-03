//! Safe recursive-descent arithmetic evaluator.
//!
//! Supports `+`, `-`, `*`, `/`, parentheses, unary +/-, and decimal numbers.
//! No `eval()` — safe against code injection.

/// Parse and evaluate an arithmetic expression safely.
pub(crate) fn safe_eval(expr: &str) -> Result<f64, String> {
    let tokens = tokenize(expr)?;
    let mut pos = 0;
    let result = parse_expr(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err(format!("Unexpected token at position {pos}"));
    }
    Ok(result)
}

#[derive(Debug, Clone)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize(expr: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' => i += 1,
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num: f64 = num_str
                    .parse()
                    .map_err(|_| format!("Invalid number: {num_str}"))?;
                tokens.push(Token::Num(num));
            }
            c => return Err(format!("Unexpected character: {c}")),
        }
    }

    Ok(tokens)
}

/// expr = term (('+' | '-') term)*
fn parse_expr(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_term(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Plus => {
                *pos += 1;
                left += parse_term(tokens, pos)?;
            }
            Token::Minus => {
                *pos += 1;
                left -= parse_term(tokens, pos)?;
            }
            _ => break,
        }
    }
    Ok(left)
}

/// term = unary (('*' | '/') unary)*
fn parse_term(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_unary(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Star => {
                *pos += 1;
                left *= parse_unary(tokens, pos)?;
            }
            Token::Slash => {
                *pos += 1;
                let right = parse_unary(tokens, pos)?;
                if right == 0.0 {
                    return Err("Division by zero".into());
                }
                left /= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

/// unary = ('-' | '+')? primary
fn parse_unary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos < tokens.len() {
        match tokens[*pos] {
            Token::Minus => {
                *pos += 1;
                Ok(-parse_primary(tokens, pos)?)
            }
            Token::Plus => {
                *pos += 1;
                parse_primary(tokens, pos)
            }
            _ => parse_primary(tokens, pos),
        }
    } else {
        Err("Unexpected end of expression".into())
    }
}

/// primary = number | '(' expr ')'
fn parse_primary(tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of expression".into());
    }

    match &tokens[*pos] {
        Token::Num(n) => {
            let val = *n;
            *pos += 1;
            Ok(val)
        }
        Token::LParen => {
            *pos += 1;
            let val = parse_expr(tokens, pos)?;
            if *pos >= tokens.len() {
                return Err("Missing closing parenthesis".into());
            }
            match tokens[*pos] {
                Token::RParen => {
                    *pos += 1;
                    Ok(val)
                }
                _ => Err("Expected closing parenthesis".into()),
            }
        }
        _ => Err(format!("Unexpected token at position {pos}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_simple_addition() {
        assert_eq!(safe_eval("2 + 3").unwrap(), 5.0);
    }

    #[test]
    fn eval_operator_precedence() {
        assert_eq!(safe_eval("2 + 3 * 4").unwrap(), 14.0);
    }

    #[test]
    fn eval_parentheses() {
        assert_eq!(safe_eval("(2 + 3) * 4").unwrap(), 20.0);
    }

    #[test]
    fn eval_nested_parens() {
        assert_eq!(safe_eval("((1 + 2) * (3 + 4))").unwrap(), 21.0);
    }

    #[test]
    fn eval_division_by_zero() {
        assert!(safe_eval("1 / 0").unwrap_err().contains("Division by zero"));
    }

    #[test]
    fn eval_negation() {
        assert_eq!(safe_eval("-5 + 3").unwrap(), -2.0);
    }

    #[test]
    fn eval_decimals() {
        assert_eq!(safe_eval("3.5 * 2").unwrap(), 7.0);
    }
}
