use std::fmt;
use std::time::Duration;

use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::str::Chars;

fn lines_from_file<P: AsRef<Path>>(filepath: P) -> Vec<String> {
    let file = File::open(filepath).unwrap();
    io::BufReader::new(file)
        .lines()
        .map(|l| l.expect("Could not parse line"))
        .collect()
}

// #[derive(Debug)]
pub struct TimedText {
    pub position: Duration,
    pub text: String,
}

impl fmt::Debug for TimedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TimedText {{ position: {}, text: {} }}",
            self.position.as_micros(),
            self.text
        )
    }
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
    pub timed_texts: Vec<TimedText>,
}

fn duration_from_time_string(chars: &mut std::iter::Iterator<Item = char>) -> Duration {
    let minutes_str: String = chars.take(2).collect();
    let minutes = u64::from_str_radix(&minutes_str, 10).expect("Bad minutes format");

    if chars.next() != Some(':') {
        panic!("Bad time format");
    }
    let seconds_str: String = chars.take(2).collect();
    let seconds = u64::from_str_radix(&seconds_str, 10).expect("Bad seconds format");

    if chars.next() != Some('.') {
        panic!("Bad time format");
    }
    let centiseconds_str: String = chars.take(2).collect();
    let centiseconds = u64::from_str_radix(&centiseconds_str, 10).expect("Bad centiseconds format");

    Duration::from_micros(((((minutes * 60) + seconds) * 100) + centiseconds) * 10000)
}

fn parse_lrc_line(chars: &mut std::iter::Peekable<Chars>) -> LrcLine {
    match chars.next() {
        None => LrcLine::Empty,
        Some('[') => {
            let first_char_in_tag_name = chars.peek().expect("Invalid lrc file format");
            if first_char_in_tag_name.is_ascii_digit() {
                let position = duration_from_time_string(&mut chars.take_while(|c| *c != ']'));
                if chars.next() != Some(']') {
                    panic!("Invalid lrc file format");
                }
                LrcLine::TimedText(TimedText {
                    position,
                    text: chars.collect(),
                })
            } else {
                LrcLine::Unknown
            }
        }
        _ => {
            panic!("Invalid lrc file format");
        }
    }
}

pub fn parse_lrc_file<P: AsRef<Path>>(filepath: P) -> LrcFile {
    let text_lines = lines_from_file(filepath);
    let mut timed_texts = Vec::new();
    for line in text_lines {
        let mut chars = line.chars().peekable();
        let lrc_line = parse_lrc_line(&mut chars);
        if let LrcLine::TimedText(t) = lrc_line {
            timed_texts.push(t);
        }
    }
    LrcFile {
        metadata: Vec::new(),
        timed_texts,
    }
}
