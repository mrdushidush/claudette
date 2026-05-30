pub fn dedup_sorted<T: PartialEq + Clone>(items: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        match out.last() {
            Some(prev) if prev == item => {}
            _ => out.push(item.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_first_of_each_run() {
        assert_eq!(dedup_sorted(&[1, 1, 2, 2, 2, 3, 1, 1]), vec![1, 2, 3, 1]);
    }
}
