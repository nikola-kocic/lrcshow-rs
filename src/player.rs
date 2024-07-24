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
type DbusVariantRefArg = dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>;

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

fn parse_player_metadata(metadata: DbusVariantRefArg) -> Result<Option<Metadata>, String> {
    let mut file_path_uri: Option<&str> = None;

    let mut metadata_iter = metadata.as_iter().unwrap();
    while let Some(key_arg) = metadata_iter.next() {
        let key = key_arg.as_str().unwrap();
        let value_arg = metadata_iter.next().unwrap();
        if key == MPRIS2_METADATA_FILE_URI {
            let uri = value_arg.as_str().ok_or("url metadata should be string")?;
            file_path_uri = Some(uri);
        }
    }
    trace!("file_path_uri = {:#?}", file_path_uri);
    let Some(file_path_uri) = file_path_uri else {
        // If playlist has reached end, new metadata event is sent,
        // but it doesn't contain any of the following keys
        return Ok(None);
    };

    // Try parsing path as URL, if it fails, it's probably the absolute path
    let file_path = match Url::parse(file_path_uri) {
        Ok(file_path_url) => file_path_url
            .to_file_path()
            .map_err(|_| format!("invalid format of url metadata: {}", file_path_url))?,
        Err(_) => PathBuf::from(file_path_uri),
    };

    Ok(Some(Metadata { file_path }))
}

fn query_player_metadata(p: &ConnectionProxy) -> Result<Option<Metadata>, String> {
    query_player_property::<DbusVariantRefArg>(p, "Metadata").and_then(parse_player_metadata)
}

pub fn query_player_state(p: &ConnectionProxy) -> Result<PlayerState, String> {
    let playback_status = query_player_playback_status(p)?;
    let position = query_player_position(p)?;
    let instant = Instant::now();
    let metadata = if playback_status != PlaybackStatus::Stopped {
        query_player_metadata(p)?
    } else {
        None
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
        .map(|str_ref| str_ref.to_owned())
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

fn get_dbus_properties_changed_handler(
    sender: Sender<TimedEvent>,
) -> impl Fn(PropertiesPropertiesChanged, &LocalConnection, &Message) -> bool {
    move |e: PropertiesPropertiesChanged, _: &LocalConnection, _: &Message| {
        let instant = Instant::now();
        debug!("DBus.Properties happened: {:?}", e);
        if e.interface_name == "org.mpris.MediaPlayer2.Player" {
            for (k, v) in e.changed_properties {
                match k.as_ref() {
                    "PlaybackStatus" => {
                        let playback_status = v.as_str().unwrap();
                        debug!("playback_status = {:?}", playback_status);
                        sender
                            .send(TimedEvent {
                                instant,
                                event: Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(
                                    parse_playback_status(playback_status),
                                )),
                            })
                            .unwrap();
                    }
                    "Metadata" => {
                        let metadata = parse_player_metadata(v).unwrap();
                        sender
                            .send(TimedEvent {
                                instant,
                                event: Event::PlayerEvent(PlayerEvent::MetadataChange(metadata)),
                            })
                            .unwrap();
                    }
                    "Position" => {
                        let position_us = v.as_i64().unwrap();
                        sender
                            .send(TimedEvent {
                                instant,
                                event: Event::PlayerEvent(PlayerEvent::Seeked {
                                    position: Duration::from_micros(position_us as u64),
                                }),
                            })
                            .unwrap();
                    }
                    "Volume" => {}
                    _ => {
                        warn!("Unknown PropertiesChanged event:");
                        warn!("    changed_property {} = {:?}", k, v);
                        warn!(
                            "    invalidated_properties = {:?}",
                            e.invalidated_properties
                        );
                    }
                }
            }
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

fn query_player_owner_name<'a>(
    c: &'a LocalConnection,
    player: &'a str,
) -> Result<Option<String>, String> {
    let all_player_buses = query_all_player_buses(c)?;

    let player_bus = format!("{}{}", MPRIS2_PREFIX, player);
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
    let player_bus = format!("{}{}", MPRIS2_PREFIX, player);

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

    fn run_sync(&self, player: String) {
        let (tx, rx) = channel::<PlayerLifetimeEvent>();
        let c = LocalConnection::new_session().unwrap();
        subscribe_to_player_start_stop(&c, &player, &tx).unwrap();
        tx.send(PlayerLifetimeEvent::PlayerStarted).unwrap();
        loop {
            loop {
                match rx.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    Ok(PlayerLifetimeEvent::PlayerStarted) => {
                        if let Some(player_owner_name) =
                            query_player_owner_name(&c, &player).unwrap()
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
            self.run_sync(player_string);
        });
    }
}
