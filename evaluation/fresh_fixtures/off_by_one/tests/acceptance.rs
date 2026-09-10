use off_by_one::sum_up_to;

#[test]
fn sums_inclusive_range() {
    assert_eq!(sum_up_to(0), 0);
    assert_eq!(sum_up_to(1), 1);
    assert_eq!(sum_up_to(5), 15);
}
