pub fn clamp(val: i64, min: i64, max: i64) -> i64 {
    if val < min {
        max
    } else if val > max {
        min
    } else {
        val
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_values_correctly() {
        assert_eq!(clamp(5, 0, 10), 5);
        assert_eq!(clamp(-5, 0, 10), 0);
        assert_eq!(clamp(15, 0, 10), 10);
    }
}
