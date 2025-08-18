// Copyright 2025 Martin Pool

//! Concise human-readable representation of time durations.

use std::time::Duration;

/// Formats a duration as a short string, containing hours, minutes, seconds,
/// but only if they are non-zero. Only include seconds for shorter durations.
pub fn format(duration: Duration) -> String {
    let mut parts = Vec::with_capacity(2);
    let mut r = (duration.as_millis() + 500) / 1000;
    if r == 0 {
        return "0s".to_string();
    }

    if r >= 3600 * 24 {
        parts.push(format!("{}d", r / (3600 * 24)));
        r %= 3600 * 24;
    }
    if r >= 3600 {
        parts.push(format!("{}h", r / 3600));
        r %= 3600;
    }
    if r >= 60 {
        parts.push(format!("{}m", r / 60));
        r %= 60;
    }
    if parts.len() < 2 && r >= 1 {
        parts.push(format!("{}s", r));
    }

    parts.join("")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_short_time() {
        assert_eq!(format(Duration::from_millis(1)), "0s");
        assert_eq!(format(Duration::from_millis(499)), "0s"); // round down
        assert_eq!(format(Duration::from_millis(500)), "1s"); // round up
        assert_eq!(format(Duration::from_millis(999)), "1s"); // round up
        assert_eq!(format(Duration::from_millis(1000)), "1s");
        assert_eq!(format(Duration::from_millis(60000)), "1m");
        assert_eq!(format(Duration::from_millis(3600000)), "1h");
        assert_eq!(format(Duration::from_millis(3660000)), "1h1m");
        assert_eq!(format(Duration::from_millis(3661000)), "1h1m"); // seconds are omitted
        assert_eq!(format(Duration::from_secs(3600 * 7 + 5 * 60 + 30)), "7h5m");
        assert_eq!(
            format(Duration::from_secs(3600 * 24 * 7 + 3600 * 7 + 5 * 60 + 30)),
            "7d7h5m"
        );
        assert_eq!(
            format(Duration::from_secs(3600 * 24 * 7 + 5 * 60 + 30)),
            "7d5m"
        );
        assert_eq!(format(Duration::from_secs(3600 * 24 * 7 + 30)), "7d30s");
    }
}
