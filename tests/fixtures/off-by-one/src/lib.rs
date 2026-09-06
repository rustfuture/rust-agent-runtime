pub fn sum_up_to(n: u32) -> u32 {
    let mut sum = 0;
    // BUG: should be <= n
    for i in 1..=n {
        sum += i;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sum() {
        assert_eq!(sum_up_to(0), 0);
        assert_eq!(sum_up_to(1), 1);
        assert_eq!(sum_up_to(5), 15);
    }
}
