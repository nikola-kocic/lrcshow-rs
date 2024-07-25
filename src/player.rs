use std::convert::TryFrom;
use std::convert::TryInto;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::RefArg;
use dbus::blocking::stdintf::org_freedesktop_dbus::{Properties, PropertiesPropertiesChanged};
use dbus::blocking::BlockingSender;
use dbus::blocking::{LocalConnection, Proxy};
use dbus::{arg, Message};
use url::Url;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::events::{
    Event, Metadata, PlaybackStatus, PlayerEvent, PlayerState, PositionSnapshot, TimedEvent,
};

const MPRIS2_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS2_PATH: &str = "/org/mpris/MediaPlayer2";

const MPRIS2_METADATA_FILE_URI: &str = "xesam:url";

pub type ConnectionProxy<'a> = Proxy<'a, &'a LocalConnection>;

fn query_player_property<T>(p: &ConnectionProxy, name: &str) -> Result<T, String>
where
    for<'b> T: dbus::arg::Get<'b> + 'static,
{
    p.get("org.mpris.MediaPlayer2.Player", name)
        .map_err(|e| e.to_string())
}

pub fn query_player_position(p: &ConnectionProxy) -> Result<Duration, String> {
    let v = query_player_property::<i64>(p, "Position")?;
    if v < 0 {
        panic!("Wrong position value");
    }
    Ok(Duration::from_micros(v.try_into().unwrap()))
}

fn query_player_playback_status(p: &ConnectionProxy) -> Result<PlaybackStatus, String> {
    query_player_property::<String>(p, "PlaybackStatus").map(|v| parse_playback_status(&v))
}

fn parse_player_metadata(metadata: &dyn RefArg) -> Result<Option<Metadata>, String> {
    let mut file_path_uri: Option<&str> = None;
    debug!("parse_player_metadata");

    let mut metadata_iter = metadata
        .as_iter()
        .ok_or("metadata should be an a{sv} map")?;
    while let Some(key_arg) = metadata_iter.next() {
        debug!("key = {key_arg:#?}");
        let key = key_arg.as_str().ok_or(format!(
            "metadata key should be a string, found: {key_arg:?}"
        ))?;
        let value_arg = metadata_iter
            .next()
            .ok_or(format!("metadata value for {key} cannot be read"))?;
        debug!("key = {key}, value = {value_arg:#?}");
        if key == MPRIS2_METADATA_FILE_URI {
            let uri = value_arg.as_str().ok_or(format!(
                "url metadata should be string, found {value_arg:?}"
            ))?;
            file_path_uri = Some(uri);
        }
    }
    trace!("file_path_uri = {file_path_uri:#?}");
    let Some(file_path_uri) = file_path_uri else {
        // If playlist has reached end, new metadata event is sent,
        // but it doesn't contain any of the following keys
        return Ok(None);
    };

    // Try parsing path as URL, if it fails, it's probably the absolute path
    let file_path = match Url::parse(file_path_uri) {
        Ok(file_path_url) => file_path_url
            .to_file_path()
            .map_err(|()| format!("invalid format of url metadata: {file_path_url}"))?,
        Err(_) => PathBuf::from(file_path_uri),
    };

    Ok(Some(Metadata { file_path }))
}

fn query_player_metadata(p: &ConnectionProxy) -> Result<Option<Metadata>, String> {
    query_player_property::<Box<dyn RefArg>>(p, "Metadata").and_then(|m| parse_player_metadata(&m))
}

pub fn query_player_state(p: &ConnectionProxy) -> Result<PlayerState, String> {
    let playback_status = query_player_playback_status(p)?;
    let position = query_player_position(p)?;
    let instant = Instant::now();
    let metadata = if playback_status == PlaybackStatus::Stopped {
        None
    } else {
        query_player_metadata(p)?
    };
    Ok(PlayerState {
        playback_status,
        position_snapshot: PositionSnapshot { position, instant },
        metadata,
    })
}

fn parse_playback_status(playback_status: &str) -> PlaybackStatus {
    match playback_status {
        "Playing" => PlaybackStatus::Playing,
        "Paused" => PlaybackStatus::Paused,
        "Stopped" => PlaybackStatus::Stopped,
        _ => panic!(""),
    }
}

fn query_unique_owner_name(c: &LocalConnection, bus_name: &str) -> Result<String, String> {
    let get_name_owner = Message::new_method_call(
        "org.freedesktop.DBus",
        "/",
        "org.freedesktop.DBus",
        "GetNameOwner",
    )?
    .append1(bus_name);

    c.send_with_reply_and_block(get_name_owner, Duration::from_millis(100))
        .map_err(|e| e.to_string())
        .map(|reply| {
            reply
                .get1()
                .expect("GetNameOwner must have name as first member")
        })
}

fn query_all_player_buses(c: &LocalConnection) -> Result<Vec<String>, String> {
    let list_names = Message::new_method_call(
        "org.freedesktop.DBus",
        "/",
        "org.freedesktop.DBus",
        "ListNames",
    )?;

    let reply = c
        .send_with_reply_and_block(list_names, Duration::from_millis(500))
        .map_err(|e| e.to_string())?;

    let names: arg::Array<&str, _> = reply.read1().map_err(|e| e.to_string())?;

    Ok(names
        .filter(|name| name.starts_with(MPRIS2_PREFIX))
        .map(std::borrow::ToOwned::to_owned)
        .collect())
}

#[derive(Debug)]
pub struct MediaPlayer2SeekedHappened {
    pub position_us: i64,
}

impl dbus::message::SignalArgs for MediaPlayer2SeekedHappened {
    const NAME: &'static str = "Seeked";
    const INTERFACE: &'static str = "org.mpris.MediaPlayer2.Player";
}

impl arg::ReadAll for MediaPlayer2SeekedHappened {
    fn read(i: &mut arg::Iter) -> Result<Self, arg::TypeMismatchError> {
        Ok(Self {
            position_us: i.read()?,
        })
    }
}

#[derive(Debug)]
pub struct DbusNameOwnedChanged {
    pub name: String,
    pub new_owner: String,
    pub old_owner: String,
}

impl dbus::message::SignalArgs for DbusNameOwnedChanged {
    const NAME: &'static str = "NameOwnerChanged";
    const INTERFACE: &'static str = "org.freedesktop.DBus";
}

impl arg::ReadAll for DbusNameOwnedChanged {
    fn read(i: &mut arg::Iter) -> Result<Self, arg::TypeMismatchError> {
        Ok(Self {
            name: i.read()?,
            new_owner: i.read()?,
            old_owner: i.read()?,
        })
    }
}

pub fn get_connection_proxy<'a>(
    c: &'a LocalConnection,
    player_owner_name: &'a str,
) -> ConnectionProxy<'a> {
    debug!("get_connection_proxy with {}", player_owner_name);
    c.with_proxy(player_owner_name, MPRIS2_PATH, Duration::from_millis(5000))
}

fn get_mediaplayer2_seeked_handler(
    sender: Sender<TimedEvent>,
) -> impl Fn(MediaPlayer2SeekedHappened, &LocalConnection, &Message) -> bool {
    move |e: MediaPlayer2SeekedHappened, _: &LocalConnection, _: &Message| {
        let instant = Instant::now();
        debug!("Seek happened: {:?}", e);
        if e.position_us < 0 {
            panic!(
                "Position value must be positive number, found {}",
                e.position_us
            );
        }
        sender
            .send(TimedEvent {
                instant,
                event: Event::PlayerEvent(PlayerEvent::Seeked {
                    position: Duration::from_micros(e.position_us as u64),
                }),
            })
            .unwrap();
        true
    }
}

fn react_on_changed_properties<F: FnMut(PlayerEvent)>(
    changed_properties: dbus::arg::PropMap,
    mut f: F,
) {
    debug!("react_on_changed_properties: {changed_properties:?}");
    for (k, v) in changed_properties {
        match k.as_ref() {
            "PlaybackStatus" => {
                let playback_status_str = v.as_str().unwrap();
                debug!("playback_status = {:?}", playback_status_str);
                let playback_status = parse_playback_status(playback_status_str);
                f(PlayerEvent::PlaybackStatusChange(playback_status))
            }
            "Metadata" => {
                let metadata = parse_player_metadata(&v.0).unwrap();
                f(PlayerEvent::MetadataChange(metadata));
            }
            "Position" => {
                let position_us = v.as_i64().unwrap();
                let position = Duration::from_micros(u64::try_from(position_us).unwrap());
                f(PlayerEvent::Seeked { position });
            }
            "Volume" => {}
            _ => {
                f(PlayerEvent::Unknown {
                    key: k,
                    value: format!("{:?}", v),
                });
            }
        }
    }
}

fn get_dbus_properties_changed_handler(
    sender: Sender<TimedEvent>,
) -> impl Fn(PropertiesPropertiesChanged, &LocalConnection, &Message) -> bool {
    move |e: PropertiesPropertiesChanged, _: &LocalConnection, _: &Message| {
        let instant = Instant::now();
        if e.interface_name == "org.mpris.MediaPlayer2.Player" {
            react_on_changed_properties(e.changed_properties, |player_event| {
                sender
                    .send(TimedEvent {
                        instant,
                        event: Event::PlayerEvent(player_event),
                    })
                    .unwrap();
            });
        }
        true
    }
}

enum PlayerLifetimeEvent {
    PlayerStarted,
    PlayerShutDown,
}

fn get_dbus_name_owned_changed_handler(
    sender: Sender<PlayerLifetimeEvent>,
    player_bus: String,
) -> impl Fn(DbusNameOwnedChanged, &LocalConnection, &Message) -> bool {
    move |e: DbusNameOwnedChanged, _: &LocalConnection, _: &Message| {
        // debug!("DbusNameOwnedChanged happened: {:?}", e);
        if e.name == player_bus && e.old_owner.is_empty() {
            sender.send(PlayerLifetimeEvent::PlayerShutDown).unwrap();
        } else if e.name == player_bus && e.new_owner.is_empty() {
            sender.send(PlayerLifetimeEvent::PlayerStarted).unwrap();
        }
        true
    }
}

fn query_player_owner_name(c: &LocalConnection, player: &str) -> Result<Option<String>, String> {
    let all_player_buses = query_all_player_buses(c)?;

    let player_bus = format!("{MPRIS2_PREFIX}{player}");
    if !all_player_buses.contains(&player_bus) {
        info!(
            "Specified player not running. Found the following players: {}",
            all_player_buses
                .iter()
                .map(|s| s.trim_start_matches(MPRIS2_PREFIX))
                .collect::<Vec<&str>>()
                .join(", ")
        );
        return Ok(None);
    }

    let player_owner_name = query_unique_owner_name(c, &player_bus)?;
    debug!("player_owner_name = {:?}", player_owner_name);
    Ok(Some(player_owner_name))
}

fn subscribe<'a>(
    c: &'a LocalConnection,
    player_owner_name: &'a str,
    sender: &Sender<TimedEvent>,
) -> Result<(), String> {
    let p = get_connection_proxy(c, player_owner_name);

    p.match_signal(get_dbus_properties_changed_handler(sender.clone()))
        .map_err(|e| e.to_string())?;

    p.match_signal(get_mediaplayer2_seeked_handler(sender.clone()))
        .map_err(|e| e.to_string())?;

    // p.match_signal(|_: MediaPlayer2TrackListChangeHappened, _: &Connection| {
    //     debug!("TrackList happened");
    //     true
    // }).map_err(|e| e.to_string())?;

    Ok(())
}

fn subscribe_to_player_start_stop(
    c: &LocalConnection,
    player: &str,
    sender: &Sender<PlayerLifetimeEvent>,
) -> Result<(), String> {
    let player_bus = format!("{MPRIS2_PREFIX}{player}");

    let proxy_generic_dbus = c.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_millis(5000),
    );
    proxy_generic_dbus
        .match_signal(get_dbus_name_owned_changed_handler(
            sender.clone(),
            player_bus,
        ))
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub struct PlayerNotifications {
    sender: Sender<TimedEvent>,
}

impl PlayerNotifications {
    pub fn new(sender: Sender<TimedEvent>) -> Self {
        Self { sender }
    }

    fn run_sync(&self, player: &str) {
        let (tx, rx) = channel::<PlayerLifetimeEvent>();
        let c = LocalConnection::new_session().unwrap();
        subscribe_to_player_start_stop(&c, player, &tx).unwrap();
        tx.send(PlayerLifetimeEvent::PlayerStarted).unwrap();
        loop {
            loop {
                match rx.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    Ok(PlayerLifetimeEvent::PlayerStarted) => {
                        if let Some(player_owner_name) =
                            query_player_owner_name(&c, player).unwrap()
                        {
                            let instant = Instant::now();
                            subscribe(&c, &player_owner_name, &self.sender).unwrap();
                            self.sender
                                .send(TimedEvent {
                                    instant,
                                    event: Event::PlayerEvent(PlayerEvent::PlayerStarted {
                                        player_owner_name,
                                    }),
                                })
                                .unwrap();
                        }
                    }
                    Ok(PlayerLifetimeEvent::PlayerShutDown) => {
                        let instant = Instant::now();
                        self.sender
                            .send(TimedEvent {
                                instant,
                                event: Event::PlayerEvent(PlayerEvent::PlayerShutDown),
                            })
                            .unwrap();
                    }
                }
            }
            c.process(Duration::from_millis(16)).unwrap();
        }
    }

    pub fn run_async(self, player: &str) {
        let player_string = player.to_owned();
        thread::spawn(move || {
            self.run_sync(&player_string);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use dbus::arg::Variant;
    use test_log::test;

    #[test]
    fn test_regular() {
        // Message {
        //     Type: Signal,
        //     Path: "/org/mpris/MediaPlayer2",
        //     Interface: "org.freedesktop.DBus.Properties",
        //     Member: "PropertiesChanged",
        //     Sender: ":1.154",
        //     Serial: 13,
        //     Args: [
        //         "org.mpris.MediaPlayer2.Player",
        //         {
        //             "Metadata": Variant(
        //                 {
        //                     "mpris:artUrl": Variant("file:///tmp/audacious-temp-75WRR2"),
        //                     "mpris:trackid": Variant(Path("/org/mpris/MediaPlayer2/CurrentTrack\0")),
        //                     "xesam:artist": Variant(["Queen"]),
        //                     "xesam:album": Variant("Greatest Hits II"),
        //                     "xesam:url": Variant("file:///home/user/music/Queen/--%20Compilations%20--/%281991%29%20Greatest%20Hits%20II/13%20Queen%20-%20The%20Invisible%20Man.mp3"),
        //                     "mpris:length": Variant(238655000),
        //                     "xesam:title": Variant("The Invisible Man")
        //                 }
        //             )
        //         },
        //         []
        //     ]
        // }

        let mut test_changed_metadata = dbus::arg::PropMap::new();
        test_changed_metadata.insert(
            "mpris:artUrl".to_owned(),
            Variant(Box::new("file:///tmp/audacious-temp-75WRR2".to_string())),
        );
        test_changed_metadata.insert(
            "mpris:trackid".to_owned(),
            Variant(Box::new(
                dbus::strings::Path::from_slice("/org/mpris/MediaPlayer2/CurrentTrack\0").unwrap(),
            )),
        );
        test_changed_metadata.insert(
            "xesam:artist".to_owned(),
            Variant(Box::new(vec!["Queen".to_owned()])),
        );
        test_changed_metadata.insert(
            "xesam:album".to_owned(),
            Variant(Box::new("Greatest Hits II".to_owned())),
        );

        test_changed_metadata.insert(
            "xesam:url".to_owned(),
            Variant(Box::new(
                "file:///home/user/music/Queen/--%20Compilations%20--/%281991%29%20Greatest%20Hits%20II/13%20Queen%20-%20The%20Invisible%20Man.mp3".to_owned()
            ))
        );
        test_changed_metadata.insert("mpris:length".to_owned(), Variant(Box::new(238655000)));

        test_changed_metadata.insert(
            "xesam:title".to_owned(),
            Variant(Box::new("The Invisible Man".to_owned())),
        );

        let mut test_changed_properties = dbus::arg::PropMap::new();
        test_changed_properties.insert(
            "Metadata".to_owned(),
            Variant(Box::new(test_changed_metadata)),
        );

        let mut reported_events = Vec::<PlayerEvent>::new();
        react_on_changed_properties(test_changed_properties, |player_event| {
            reported_events.push(player_event)
        });

        assert_eq!(reported_events, vec![
            PlayerEvent::MetadataChange(Some(Metadata {
                file_path: PathBuf::from_str(
                    "/home/user/music/Queen/-- Compilations --/(1991) Greatest Hits II/13 Queen - The Invisible Man.mp3"
                ).unwrap()
            }))
        ]);
    }
}
