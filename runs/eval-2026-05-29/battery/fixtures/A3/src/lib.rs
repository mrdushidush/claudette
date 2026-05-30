pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

pub fn sub(a: i64, b: i64) -> i64 {
    a - b
}

pub fn mul(a: i64, b: i64) -> i64 {
    a * b
}

pub fn double(a: i64) -> i64 {
    a * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn test_sub() {
        assert_eq!(sub(5, 3), 2);
    }

    #[test]
    fn test_mul() {
        assert_eq!(mul(4, 3), 12);
    }

    #[test]
    fn test_double() {
        assert_eq!(double(7), 14);
    }
}
