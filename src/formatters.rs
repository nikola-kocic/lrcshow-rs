use std::time::Duration;

// Formats duration to the minutes, seconds and milliseconds format `01:02.55`
pub fn format_duration(duration: &Duration) -> String {
    let total_seconds = duration.as_secs_f64();
    let minutes = ((total_seconds / 60.0).floor() as i32) % 60;
    let seconds = total_seconds - f64::from(minutes * 60);
    format!("{:02}:{:05.2}", minutes, seconds)
}
