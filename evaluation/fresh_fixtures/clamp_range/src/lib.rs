/// Clamp `value` into the inclusive range `[min, max]`.
pub fn clamp(value: i32, min: i32, max: i32) -> i32 {
    if value < min {
        max
    } else if value > max {
        min
    } else {
        value
    }
}
