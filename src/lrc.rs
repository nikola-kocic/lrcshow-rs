use std::fmt;
use std::time::Duration;

use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::str::Chars;

fn lines_from_file<P: AsRef<Path>>(filepath: P) -> Result<Vec<String>, String> {
    let file = File::open(filepath).map_err(|e| e.to_string())?;
    Ok(io::BufReader::new(file)
        .lines()
        .map(|l| l.expect("Could not parse line"))
        .collect())
}

pub struct TimedLocation {
    pub time: Duration,
    pub line_char_from_index: i32, // from this character in line
    pub line_char_to_index: i32,   // to this character in line
}

impl fmt::Debug for TimedLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TimedLocation {{ time: {}, from: {}, to: {} }}",
            self.time.as_micros(),
            self.line_char_from_index,
            self.line_char_to_index,
        )
    }
}

#[derive(Debug)]
pub struct TimedText {
    pub text: String,
    pub timings: Vec<TimedLocation>,
}

#[derive(Debug)]
enum LrcLine {
    Empty,
    Unknown,
    TimedText(TimedText),
}

#[derive(Debug)]
pub struct LrcFile {
    metadata: Vec<(String, String)>,
    pub timed_texts_lines: Vec<TimedText>,
}

fn duration_from_time_string(
    chars: &mut dyn std::iter::Iterator<Item = char>,
) -> Result<Duration, String> {
    let minutes_str: String = chars.take(2).collect();
    let minutes = u64::from_str_radix(&minutes_str, 10).expect("Bad minutes format");

    if chars.next() != Some(':') {
        return Err(String::from("Bad time format"));
    }
    let seconds_str: String = chars.take(2).collect();
    let seconds = u64::from_str_radix(&seconds_str, 10).expect("Bad seconds format");

    if chars.next() != Some('.') {
        return Err(String::from("Bad time format"));
    }
    let centiseconds_str: String = chars.take(2).collect();
    let centiseconds = u64::from_str_radix(&centiseconds_str, 10).expect("Bad centiseconds format");

    Ok(Duration::from_micros(
        ((((minutes * 60) + seconds) * 100) + centiseconds) * 10000,
    ))
}

fn parse_lrc_line(chars: &mut std::iter::Peekable<Chars>) -> Result<LrcLine, String> {
    match chars.next() {
        None => Ok(LrcLine::Empty),
        Some('[') => {
            let first_char_in_tag_name = chars.peek().expect("Invalid lrc file format");
            if first_char_in_tag_name.is_ascii_digit() {
                let time = duration_from_time_string(&mut chars.take_while(|c| *c != ']'))?;
                if chars.next() != Some(']') {
                    return Err(String::from("Invalid lrc file format"));
                }
                let text: String = chars.collect();
                let text_len = text.chars().count() as i32;
                let mut timings = Vec::new();
                let location = TimedLocation {
                    time,
                    line_char_from_index: 0,
                    line_char_to_index: text_len,
                };
                timings.push(location);
                Ok(LrcLine::TimedText(TimedText { text, timings }))
            } else {
                Ok(LrcLine::Unknown)
            }
        }
        _ => Err(String::from("Invalid lrc file format")),
    }
}

pub fn parse_lrc_file<P: AsRef<Path>>(filepath: P) -> Result<LrcFile, String> {
    let text_lines = lines_from_file(filepath)?;
    let mut timed_texts_lines = Vec::new();
    for line in text_lines {
        let mut chars = line.chars().peekable();
        let lrc_line = parse_lrc_line(&mut chars)?;
        if let LrcLine::TimedText(t) = lrc_line {
            timed_texts_lines.push(t);
        }
    }
    Ok(LrcFile {
        metadata: Vec::new(),
        timed_texts_lines,
    })
}
