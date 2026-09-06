pub fn get_first_even(numbers: &[i32]) -> Option<i32> {
    numbers.iter().copied().find(|&x| x % 2 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_first_even() {
        assert_eq!(get_first_even(&[1, 3, 4, 5]), Some(4));
    }
    
    #[test]
    fn test_get_first_even_none() {
        let result = get_first_even(&[1, 3, 5]);
        assert_eq!(result, None);
    }
}
