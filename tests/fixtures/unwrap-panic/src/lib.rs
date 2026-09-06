pub fn get_first_even(numbers: &[i32]) -> i32 {
    // BUG: panics if no even number is found
    *numbers.iter().find(|&&x| x % 2 == 0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_first_even() {
        assert_eq!(get_first_even(&[1, 3, 4, 5]), 4);
    }
    
    #[test]
    fn test_get_first_even_none() {
        // This test will fail due to panic
        // The function should return an Option<i32> instead of panicking
        let result = get_first_even(&[1, 3, 5]);
        // This assertion won't even be reached currently
        // assert_eq!(result, None);
    }
}
