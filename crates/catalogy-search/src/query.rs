use catalogy_core::{MediaType, SearchFilters, SearchQuery};
use chrono::NaiveDate;

/// Parse a user query string into a SearchQuery.
///
/// Supported filter prefixes:
/// - `type:image` / `type:video` - media type filter
/// - `after:YYYY-MM-DD` - date filter (after)
/// - `before:YYYY-MM-DD` - date filter (before)
///
/// Everything else becomes the semantic text query.
pub fn parse_query(input: &str, limit: usize) -> SearchQuery {
    let mut filters = SearchFilters::default();
    let mut text_parts: Vec<&str> = Vec::new();

    for token in input.split_whitespace() {
        if let Some(value) = token.strip_prefix("type:") {
            match value.to_lowercase().as_str() {
                "image" => filters.media_type = Some(MediaType::Image),
                "video" => filters.media_type = Some(MediaType::Video),
                _ => text_parts.push(token), // Unknown type, treat as text
            }
        } else if let Some(value) = token.strip_prefix("after:") {
            if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
                filters.after = Some(date.and_hms_opt(0, 0, 0).unwrap());
            } else {
                text_parts.push(token);
            }
        } else if let Some(value) = token.strip_prefix("before:") {
            if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
                filters.before = Some(date.and_hms_opt(23, 59, 59).unwrap());
            } else {
                text_parts.push(token);
            }
        } else {
            text_parts.push(token);
        }
    }

    SearchQuery {
        text: text_parts.join(" "),
        filters,
        limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_text() {
        let q = parse_query("sunset over ocean", 20);
        assert_eq!(q.text, "sunset over ocean");
        assert_eq!(q.limit, 20);
        assert!(q.filters.media_type.is_none());
        assert!(q.filters.after.is_none());
        assert!(q.filters.before.is_none());
    }

    #[test]
    fn test_parse_type_image() {
        let q = parse_query("type:image flowers", 10);
        assert_eq!(q.text, "flowers");
        assert_eq!(q.filters.media_type, Some(MediaType::Image));
    }

    #[test]
    fn test_parse_type_video() {
        let q = parse_query("type:video surfing waves", 10);
        assert_eq!(q.text, "surfing waves");
        assert_eq!(q.filters.media_type, Some(MediaType::Video));
    }

    #[test]
    fn test_parse_type_case_insensitive() {
        let q = parse_query("type:VIDEO clouds", 10);
        assert_eq!(q.filters.media_type, Some(MediaType::Video));
        assert_eq!(q.text, "clouds");
    }

    #[test]
    fn test_parse_unknown_type_treated_as_text() {
        let q = parse_query("type:audio music", 10);
        assert!(q.filters.media_type.is_none());
        assert_eq!(q.text, "type:audio music");
    }

    #[test]
    fn test_parse_after_date() {
        let q = parse_query("after:2026-01-01 winter", 10);
        assert_eq!(q.text, "winter");
        let after = q.filters.after.unwrap();
        assert_eq!(after.date(), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    }

    #[test]
    fn test_parse_before_date() {
        let q = parse_query("before:2025-12-31 autumn", 10);
        assert_eq!(q.text, "autumn");
        let before = q.filters.before.unwrap();
        assert_eq!(
            before.date(),
            NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()
        );
    }

    #[test]
    fn test_parse_invalid_date_treated_as_text() {
        let q = parse_query("after:not-a-date spring", 10);
        assert!(q.filters.after.is_none());
        assert_eq!(q.text, "after:not-a-date spring");
    }

    #[test]
    fn test_parse_all_filters() {
        let q = parse_query("type:video after:2026-01-01 before:2026-06-30 sunset", 5);
        assert_eq!(q.text, "sunset");
        assert_eq!(q.filters.media_type, Some(MediaType::Video));
        assert!(q.filters.after.is_some());
        assert!(q.filters.before.is_some());
        assert_eq!(q.limit, 5);
    }

    #[test]
    fn test_parse_empty_query() {
        let q = parse_query("", 20);
        assert_eq!(q.text, "");
        assert!(q.filters.media_type.is_none());
    }

    #[test]
    fn test_parse_only_filters() {
        let q = parse_query("type:image after:2026-01-01", 20);
        assert_eq!(q.text, "");
        assert_eq!(q.filters.media_type, Some(MediaType::Image));
        assert!(q.filters.after.is_some());
    }

    #[test]
    fn test_parse_filters_anywhere_in_query() {
        let q = parse_query("beautiful type:image sunset after:2026-01-01 beach", 20);
        assert_eq!(q.text, "beautiful sunset beach");
        assert_eq!(q.filters.media_type, Some(MediaType::Image));
        assert!(q.filters.after.is_some());
    }

    #[test]
    fn test_parse_preserves_limit() {
        let q = parse_query("cats", 42);
        assert_eq!(q.limit, 42);
    }

    #[test]
    fn test_parse_whitespace_handling() {
        let q = parse_query("  lots   of   spaces  ", 10);
        assert_eq!(q.text, "lots of spaces");
    }
}
