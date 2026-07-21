//! Turn an input string into a flat list of tokens.

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Num(i64),
    Plus,
    Minus,
    Star,
    Slash,
}

/// Tokenize `input`. Whitespace is skipped; unknown characters are ignored.
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '0'..='9' => {
                let mut n: i64 = 0;
                while let Some(&d) = chars.peek() {
                    if let Some(digit) = d.to_digit(10) {
                        n = n * 10 + digit as i64;
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Num(n));
            }
            _ => {
                chars.next();
            }
        }
    }
    tokens
}
