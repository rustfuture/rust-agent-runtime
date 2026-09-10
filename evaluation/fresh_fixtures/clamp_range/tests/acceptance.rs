use clamp_range::clamp;

#[test]
fn clamps_into_range() {
    assert_eq!(clamp(-5, 0, 10), 0);
    assert_eq!(clamp(15, 0, 10), 10);
    assert_eq!(clamp(5, 0, 10), 5);
}
