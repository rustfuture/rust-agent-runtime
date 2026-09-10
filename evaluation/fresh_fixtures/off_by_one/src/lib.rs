/// Return the sum of the integers from 1 through `n` inclusive.
pub fn sum_up_to(n: u32) -> u32 {
    let mut sum = 0;
    for i in 1..n {
        sum += i;
    }
    sum
}
