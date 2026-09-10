use prefix_format::label;

#[test]
fn prefixes_trimmed_name() {
    assert_eq!(label("id-", " abc "), "id-abc");
    assert_eq!(label("x", "y"), "xy");
}
