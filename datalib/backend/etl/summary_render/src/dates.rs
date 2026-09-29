//! Dates as a summary groups them.

/// The year a stamp names, from its leading digits; a value that does
/// not start with a year is counted as undated rather than guessed at.
pub fn year_of(stamp: Option<&str>) -> String {
    match stamp.and_then(|s| s.get(..4)) {
        Some(y) if y.bytes().all(|b| b.is_ascii_digit()) => y.to_string(),
        _ => "(undated)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_year_is_read_off_the_front_or_not_at_all() {
        assert_eq!(year_of(Some("2364-03-12T09:15:00-07:00")), "2364");
        assert_eq!(year_of(Some("2364:03:12 09:15:00")), "2364");
        assert_eq!(year_of(Some("March 2364")), "(undated)");
        assert_eq!(year_of(None), "(undated)");
    }
}
