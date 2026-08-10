use unicode_segmentation::UnicodeSegmentation;

pub fn normalize_display_name(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_owned();
    if normalized.is_empty() {
        return Err("宠物名称不能为空".into());
    }
    if normalized.chars().any(|character| {
        character.is_control() || matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
    }) {
        return Err("宠物名称不能包含换行或控制字符".into());
    }
    let count = normalized.graphemes(true).count();
    if !(1..=20).contains(&count) {
        return Err("宠物名称必须为 1 到 20 个字符".into());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_by_unicode_grapheme_cluster() {
        assert_eq!(normalize_display_name("  奶糖  ").unwrap(), "奶糖");
        assert_eq!(normalize_display_name("👨‍👩‍👧‍👦").unwrap(), "👨‍👩‍👧‍👦");
        assert!(normalize_display_name("\n").is_err());
        assert!(normalize_display_name(&"猫".repeat(21)).is_err());
    }

    #[test]
    fn rejects_embedded_control_and_line_separator_characters() {
        assert!(normalize_display_name("奶\u{0000}糖").is_err());
        assert!(normalize_display_name("奶\u{2028}糖").is_err());
        assert!(normalize_display_name("奶\u{2029}糖").is_err());
    }
}
