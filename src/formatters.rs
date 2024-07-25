use std::time::Duration;

const SECS_PER_MINUTE: f32 = 60.0;

// Formats duration to the minutes, seconds and milliseconds format `01:02.55`
pub fn format_duration(duration: &Duration) -> String {
    let total_seconds = duration.as_secs_f32();
    let minutes = (total_seconds / SECS_PER_MINUTE).round();
    let seconds = total_seconds % SECS_PER_MINUTE;
    format!("{minutes:02}:{seconds:05.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regular() {
        assert_eq!(format_duration(&Duration::from_millis(62550)), "01:02.55");
    }

    #[test]
    fn test_zero_duration() {
        assert_eq!(format_duration(&Duration::from_millis(0)), "00:00.00");
    }

    #[test]
    fn test_more_than_an_hour() {
        assert_eq!(format_duration(&Duration::from_millis(3720910)), "62:00.91");
    }

    #[test]
    fn test_round_up() {
        assert_eq!(format_duration(&Duration::from_millis(96)), "00:00.10");
    }

    #[test]
    fn test_round_down() {
        assert_eq!(format_duration(&Duration::from_millis(164)), "00:00.16");
    }
}
