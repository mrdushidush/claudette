//! A tiny integer arithmetic evaluator for `+ - * /` with the usual precedence.
//!
//! The string is turned into tokens by [`lexer`], then reduced by [`eval`].

pub mod eval;
pub mod lexer;

pub use eval::evaluate;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_single_subtraction() {
        assert_eq!(evaluate("2 + 3 * 4"), 14);
        assert_eq!(evaluate("10 - 3"), 7);
    }
}
