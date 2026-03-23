/// Builds combined type check stubs from user-provided stubs and input variable names.
///
/// Input names that are not already declared in `user_stubs` get `Any` type annotations
/// appended, so the type checker does not report `unresolved-reference` for them.
///
/// A name is considered "already declared" only when it appears as `<name>:` (with a
/// colon immediately following), avoiding false positives from substring matches
/// (e.g. input `x` should not match `max_value: int`).
pub fn build_input_stubs(user_stubs: Option<&str>, input_names: &[String]) -> Option<String> {
    let undeclared: Vec<&String> = input_names
        .iter()
        .filter(|name| user_stubs.is_none_or(|s| !is_name_declared(s, name)))
        .collect();

    if undeclared.is_empty() {
        return user_stubs.map(String::from);
    }

    let mut stubs = String::new();
    if let Some(user) = user_stubs {
        stubs.push_str(user);
        if !stubs.ends_with('\n') {
            stubs.push('\n');
        }
    }
    stubs.push_str("from typing import Any\n");
    for name in &undeclared {
        stubs.push_str(name);
        stubs.push_str(": Any\n");
    }
    Some(stubs)
}

/// Checks whether `name` is declared as a variable in the stubs text.
///
/// Looks for `<name>:` where `name` is preceded by a line start or whitespace,
/// preventing substring false positives (e.g. `x` matching inside `max_value`).
fn is_name_declared(stubs: &str, name: &str) -> bool {
    let pattern = format!("{name}:");
    for line in stubs.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&pattern) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_inputs() {
        assert_eq!(build_input_stubs(None, &[]), None);
    }

    #[test]
    fn test_inputs_no_stubs() {
        let result = build_input_stubs(None, &["x".to_string(), "y".to_string()]);
        assert_eq!(result, Some("from typing import Any\nx: Any\ny: Any\n".to_string()));
    }

    #[test]
    fn test_input_already_declared_in_stubs() {
        let result = build_input_stubs(Some("x: int = 0"), &["x".to_string()]);
        assert_eq!(result, Some("x: int = 0".to_string()));
    }

    #[test]
    fn test_substring_no_false_positive() {
        // Input `x` should NOT match `max_value: int` — it should still get a stub
        let result = build_input_stubs(Some("max_value: int = 0"), &["x".to_string()]);
        assert_eq!(
            result,
            Some("max_value: int = 0\nfrom typing import Any\nx: Any\n".to_string())
        );
    }

    #[test]
    fn test_mixed_declared_and_undeclared() {
        let result = build_input_stubs(Some("y: int = 0"), &["x".to_string(), "y".to_string()]);
        assert_eq!(result, Some("y: int = 0\nfrom typing import Any\nx: Any\n".to_string()));
    }
}
