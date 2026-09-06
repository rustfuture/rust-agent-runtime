pub fn last_index(values: &[i32]) -> Option<usize> {
    if values.is_empty() {
        None
    } else {
        Some(values.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_last_valid_index() {
        assert_eq!(last_index(&[]), None);
        assert_eq!(last_index(&[10]), Some(0));
        assert_eq!(last_index(&[10, 20, 30]), Some(2));
    }
}
