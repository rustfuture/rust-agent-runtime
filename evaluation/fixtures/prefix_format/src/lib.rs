pub fn ensure_prefix(s: &str, prefix: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with(prefix) {
        trimmed.to_string()
    } else {
        format!("{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_prefix_when_missing() {
        assert_eq!(ensure_prefix("  user  ", "id_"), "id_user");
        assert_eq!(ensure_prefix("id_admin", "id_"), "id_admin");
    }
}
