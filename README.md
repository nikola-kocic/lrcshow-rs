# lrcshow-rs
Shows the lyrics of the song. Currently only terminal output is supported.
If lyrics file is modified, new content will be used.

## Supported platforms and players
For now only supported platform is Linux.
All [MPRIS2](https://specifications.freedesktop.org/mpris-spec/latest/) players are supported.

## Integration
Player communicates via dbus. Object path is `/com/github/nikola_kocic/lrcshow_rs/Daemon` and interface is `com.github.nikola_kocic.lrcshow_rs.Daemon` .

### Signals
#### ActiveLyricsLineChanged(s)
&nbsp;&nbsp;&nbsp;&nbsp;When active lyrics line is changed.

## Installation
Project builds with the Rust stable version, using the Cargo build system.

`cargo build --release`

Resulting binary is at `./target/release/lrcshow-rs`

## Usage
```
USAGE:
    lrcshow-rs --lyrics <lyrics> --player <player>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -l, --lyrics <lyrics>    Lyrics file to use
    -p, --player <player>    Player to use
```

## Examples
```
lrcshow-rs --player audacious --lyrics '/home/user/Rick Astley - Never Gonna Give You Up.lrc'
```
