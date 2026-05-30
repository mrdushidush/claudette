use retrier::worker::attempt_count;

#[test]
fn test_retry_count() {
    assert_eq!(attempt_count(), 5);
}
