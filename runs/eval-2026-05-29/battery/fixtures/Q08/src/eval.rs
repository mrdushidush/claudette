//! Reduce a token stream to an integer, honoring `* /` over `+ -`.

use crate::lexer::{tokenize, Token};

/// Parse one multiplicative term (a run of numbers joined by `*` and `/`),
/// advancing `idx` past it. `* /` are applied left-to-right.
fn parse_term(tokens: &[Token], idx: &mut usize) -> i64 {
    let mut acc = match tokens.get(*idx) {
        Some(Token::Num(n)) => {
            *idx += 1;
            *n
        }
        _ => 0,
    };
    while let Some(op) = tokens.get(*idx) {
        match op {
            Token::Star | Token::Slash => {
                *idx += 1;
                let rhs = match tokens.get(*idx) {
                    Some(Token::Num(n)) => {
                        *idx += 1;
                        *n
                    }
                    _ => 0,
                };
                if matches!(op, Token::Star) {
                    acc *= rhs;
                } else {
                    acc /= rhs;
                }
            }
            _ => break,
        }
    }
    acc
}

/// Evaluate an arithmetic expression string.
pub fn evaluate(input: &str) -> i64 {
    let tokens = tokenize(input);
    let mut terms: Vec<i64> = Vec::new();
    let mut ops: Vec<Token> = Vec::new();
    let mut idx = 0;

    terms.push(parse_term(&tokens, &mut idx));
    while let Some(op) = tokens.get(idx) {
        match op {
            Token::Plus | Token::Minus => {
                ops.push(op.clone());
                idx += 1;
                terms.push(parse_term(&tokens, &mut idx));
            }
            _ => break,
        }
    }

    // Combine the additive terms.
    let mut result = *terms.last().unwrap();
    for k in (0..ops.len()).rev() {
        match ops[k] {
            Token::Plus => result = terms[k] + result,
            Token::Minus => result = terms[k] - result,
            _ => {}
        }
    }
    result
}
