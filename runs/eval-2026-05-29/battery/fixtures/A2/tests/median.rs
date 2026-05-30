use calc::median;

#[test]
fn test_median() {
    assert_eq!(median(&[1, 2, 3, 4]), 2.5);
    assert_eq!(median(&[1, 2, 3]), 2.0);
}
