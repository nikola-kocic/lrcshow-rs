# lrcshow-rs
Shows the lyrics of the song. Currently only py3status and terminal output is supported.

Only [Enhanced LRC format](https://en.wikipedia.org/wiki/LRC_(file_format)#Enhanced_format) for lyrics is supported.

Lyrics are read from path of file being played, but with extension replaced with `.lrc`.
For example, if `/home/user/Rick Astley - Never Gonna Give You Up.mp3` is played, expected location of lyrics file is
`/home/user/Rick Astley - Never Gonna Give You Up.lrc`.

Otherwise, `--lyrics` argument can be used to force load the lyrics (but it will be used for all songs).

If lyrics file is modified, new content will be used.

## Supported platforms and players
For now only supported platform is Linux.
All [MPRIS2](https://specifications.freedesktop.org/mpris-spec/latest/) players are supported.

## Integration
Player communicates via D-Bus.

### D-Bus Methods
Methods are on object path `/com/github/nikola_kocic/lrcshow_rs/Lyrics` and interface `com.github.nikola_kocic.lrcshow_rs.Lyrics`.

#### GetCurrentLyrics() -> a{s}
&nbsp;&nbsp;&nbsp;&nbsp;Get lyrics for current song.

### D-Bus Signals
Signals are sent from object path `/com/github/nikola_kocic/lrcshow_rs/Daemon` and interface `com.github.nikola_kocic.lrcshow_rs.Daemon`.

#### ActiveLyricsChanged()
&nbsp;&nbsp;&nbsp;&nbsp;When lyrics for current song are changed.

#### ActiveLyricsSegmentChanged(i: line_index, i: line_char_from_index, i: line_char_to_index)
&nbsp;&nbsp;&nbsp;&nbsp;When active lyrics segment is changed.

## Installation
Project builds with the Rust stable version, using the Cargo build system.

`cargo build --release`

Resulting binary is at `./target/release/lrcshow-rs`

## Usage
```
USAGE:
    lrcshow-rs [OPTIONS] --player <player>

OPTIONS:
    -l, --lyrics <lyrics>    Lyrics file to use for all songs. By default .lrc file next to audio file, with the same
                             filename, will be used, if available.
    -p, --player <player>    Player to use
```

## Examples
```
lrcshow-rs --player audacious --lyrics '/home/user/Rick Astley - Never Gonna Give You Up.lrc'
```
