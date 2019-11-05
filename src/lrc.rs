use std::fmt;
use std::time::Duration;

use std::convert::TryInto;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

use log::debug;

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

fn duration_from_time_string(time_str: &str) -> Result<Duration, String> {
    let minutes_str = &time_str[0..2];
    let minutes = u64::from_str_radix(&minutes_str, 10).expect("Bad minutes format");

    if &time_str[2..3] != ":" {
        return Err(String::from("Bad seconds divider"));
    }
    let seconds_str = &time_str[3..5];
    let seconds = u64::from_str_radix(&seconds_str, 10).expect("Bad seconds format");

    let ms_divider_char = &time_str[5..6];
    if ms_divider_char != "." && ms_divider_char != ":" {
        return Err(String::from("Bad milliseconds divider"));
    }
    let centiseconds_str = &time_str[6..8];
    let centiseconds = u64::from_str_radix(&centiseconds_str, 10).expect("Bad centiseconds format");

    Ok(Duration::from_micros(
        ((((minutes * 60) + seconds) * 100) + centiseconds) * 10000,
    ))
}

enum Tag {
    Time(std::time::Duration),
    Unknown,
}

fn parse_tag(tag_content: &str) -> Result<Tag, String> {
    debug!("Parsing tag content {}", tag_content);
    let first_char_in_tag_name = tag_content.chars().next().expect("Invalid lrc file format");
    if first_char_in_tag_name.is_ascii_digit() {
        let time = duration_from_time_string(tag_content)?;
        Ok(Tag::Time(time))
    } else {
        Ok(Tag::Unknown)
    }
}

fn parse_lrc_line(line: String) -> Result<LrcLine, String> {
    debug!("Parsing line {}", line);
    match line.chars().next() {
        None => Ok(LrcLine::Empty),
        Some('[') => {
            let mut current_text_index_in_line = 0;
            let parts = line.split('[');
            let mut timings = Vec::new();
            let mut texts = Vec::new();
            for part in parts.skip(1) {
                let mut subparts = part.split(']');
                let tag_content = subparts.next().unwrap();
                let mut text_len: i32 = 0;

                if let Some(text) = subparts.next() {
                    texts.push(text);
                    text_len = text.bytes().len().try_into().unwrap();
                }

                match parse_tag(tag_content)? {
                    Tag::Time(time) => {
                        let location = TimedLocation {
                            time,
                            line_char_from_index: current_text_index_in_line,
                            line_char_to_index: current_text_index_in_line + text_len,
                        };
                        timings.push(location);
                        current_text_index_in_line += text_len;
                    }
                    Tag::Unknown => {
                        return Ok(LrcLine::Unknown);
                    }
                }
            }
            let text = texts.join("");
            Ok(LrcLine::TimedText(TimedText { text, timings }))
        }
        Some(c) => Err(format!(
            "Invalid lrc file format. First character in line: {}",
            c
        )),
    }
}

pub fn parse_lrc_file<P: AsRef<Path>>(filepath: P) -> Result<LrcFile, String> {
    let text_lines = lines_from_file(filepath)?;
    let mut timed_texts_lines = Vec::new();
    for line in text_lines {
        let lrc_line = parse_lrc_line(line)?;
        if let LrcLine::TimedText(t) = lrc_line {
            timed_texts_lines.push(t);
        }
    }
    Ok(LrcFile {
        metadata: Vec::new(),
        timed_texts_lines,
    })
}
