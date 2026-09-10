/// Return the prefix followed by the trimmed name.
pub fn label(prefix: &str, name: &str) -> String {
    format!("{}", name.trim())
}
