use std::fmt;
use std::time::Duration;

use std::convert::TryInto;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

fn lines_from_file<P: AsRef<Path>>(filepath: P) -> Result<Vec<String>, String> {
    let file = File::open(filepath).map_err(|e| e.to_string())?;
    io::BufReader::new(file)
        .lines()
        .map(|l| l.map_err(|e| e.to_string()))
        .collect()
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
enum Tag {
    Time(std::time::Duration),
    Offset(i64), // ms
    Unknown,
}

#[derive(Debug)]
enum LrcLine {
    Empty,
    TimedText(TimedText),
    Tag(Tag),
}

#[derive(Debug)]
pub struct LrcFile {
    metadata: Vec<(String, String)>,
    pub timed_texts_lines: Vec<TimedText>,
}

fn duration_from_time_string(time_str: &str) -> Result<Duration, String> {
    let minutes_str = &time_str[0..2];
    let minutes = u64::from_str_radix(&minutes_str, 10)
        .map_err(|e| format!("Bad minutes format ({}): {}", minutes_str, e.to_string()))?;

    if &time_str[2..3] != ":" {
        return Err("Bad seconds divider".to_owned());
    }
    let seconds_str = &time_str[3..5];
    let seconds = u64::from_str_radix(&seconds_str, 10)
        .map_err(|e| format!("Bad seconds format ({}): {}", seconds_str, e.to_string()))?;

    let ms_divider_char = &time_str[5..6];
    if ms_divider_char != "." && ms_divider_char != ":" {
        return Err(format!("Bad milliseconds divider: {}", ms_divider_char));
    }
    let centiseconds_str = &time_str[6..8];
    let centiseconds = u64::from_str_radix(&centiseconds_str, 10).map_err(|e| {
        format!(
            "Bad centiseconds format ({}): {}",
            centiseconds_str,
            e.to_string()
        )
    })?;

    Ok(Duration::from_micros(
        ((((minutes * 60) + seconds) * 100) + centiseconds) * 10000,
    ))
}

fn parse_tag(tag_content: &str) -> Result<Tag, String> {
    trace!("Parsing tag content {}", tag_content);
    let first_char_in_tag_name = tag_content
        .chars()
        .next()
        .ok_or("Tag content must not be empty")?;
    if first_char_in_tag_name.is_ascii_digit() {
        let time = duration_from_time_string(tag_content)?;
        Ok(Tag::Time(time))
    } else {
        let mut parts = tag_content.split(':');
        let tag_first_part = parts
            .next()
            .expect("Should never happen; split always returns at least one element");
        match tag_first_part {
            "offset" => {
                let offset_val_str = parts.next().ok_or_else(|| {
                    format!("Wrong offset tag format (missing ':'): {}", tag_content)
                })?;
                let offset = i64::from_str_radix(&offset_val_str, 10).map_err(|e| {
                    format!("Bad offset format ({}): {}", offset_val_str, e.to_string())
                })?;
                Ok(Tag::Offset(offset))
            }
            _ => Ok(Tag::Unknown),
        }
    }
}

fn parse_lrc_line(line: String) -> Result<LrcLine, String> {
    trace!("Parsing line {}", line);
    match line.chars().next() {
        None => Ok(LrcLine::Empty),
        Some('[') => {
            let mut current_text_index_in_line = 0;
            let parts = line.split('[');
            let mut timings = Vec::new();
            let mut texts = Vec::new();
            for part in parts.skip(1) {
                let mut subparts = part.split(']');
                let tag_content = subparts
                    .next()
                    .expect("Should never happen; split always returns at least one element");
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
                    tag => return Ok(LrcLine::Tag(tag)),
                }
            }
            let text = texts.join("");
            Ok(LrcLine::TimedText(TimedText { text, timings }))
        }
        Some(c) => {
            let mut buf = [0; 10];
            Err(format!(
                "Invalid lrc file format. First character in line: \"{}\" (hex bytes: {:x?})",
                c,
                c.encode_utf8(&mut buf).as_bytes()
            ))
        }
    }
}

pub fn parse_lrc_file<P: AsRef<Path>>(filepath: P) -> Result<LrcFile, String> {
    let text_lines = lines_from_file(filepath)?;
    let mut timed_texts_lines = Vec::new();
    let mut offset_ms = 0i64;
    for line in text_lines {
        match parse_lrc_line(line)? {
            LrcLine::TimedText(mut t) => {
                if offset_ms != 0 {
                    for timing in &mut t.timings {
                        let prev_time_ms: i64 = timing.time.as_millis().try_into().unwrap();
                        timing.time =
                            Duration::from_millis((prev_time_ms + offset_ms).try_into().unwrap());
                    }
                }
                timed_texts_lines.push(t);
            }
            LrcLine::Tag(Tag::Offset(v)) => offset_ms = v,
            _ => {}
        }
    }
    Ok(LrcFile {
        metadata: Vec::new(),
        timed_texts_lines,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsTiming {
    pub time: Duration,
    pub line_index: i32,           // index of line
    pub line_char_from_index: i32, // from this character in line
    pub line_char_to_index: i32,   // to this character in line
}

#[derive(Debug)]
pub struct Lyrics {
    pub lines: Vec<String>,
    pub timings: Vec<LyricsTiming>,
}

impl Lyrics {
    pub fn new(lrc_file: LrcFile) -> Self {
        let mut lines = Vec::new();
        let mut timings = Vec::new();

        if !lrc_file.timed_texts_lines.is_empty() {
            timings.push(LyricsTiming {
                time: Duration::from_secs(0),
                line_index: 0,
                line_char_from_index: 0,
                line_char_to_index: 0,
            });
        }

        for (line_index, timed_text_line) in (0i32..).zip(lrc_file.timed_texts_lines) {
            lines.push(timed_text_line.text);
            for timing in timed_text_line.timings {
                timings.push(LyricsTiming {
                    time: timing.time,
                    line_index,
                    line_char_from_index: timing.line_char_from_index,
                    line_char_to_index: timing.line_char_to_index,
                })
            }
        }
        Lyrics { lines, timings }
    }
}
