use std::collections::HashMap;
use std::convert::TryInto;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use dbus::arg::RefArg;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::BlockingSender;
use dbus::blocking::{Connection, Proxy};
use dbus::{arg, Message};
use log::{debug, info, warn};
use url::Url;

const MPRIS2_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS2_PATH: &str = "/org/mpris/MediaPlayer2";

type DbusStringMap = HashMap<String, arg::Variant<Box<dyn arg::RefArg>>>;
pub type ConnectionProxy<'a> = Proxy<'a, &'a Connection>;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Metadata {
    album: Option<String>,
    title: String,
    artists: Option<Vec<String>>,
    file_path: PathBuf,
    length: i64,
}

impl Metadata {
    #[allow(dead_code)]
    pub fn album(&self) -> &Option<String> {
        &self.album
    }

    #[allow(dead_code)]
    pub fn title(&self) -> &String {
        &self.title
    }

    #[allow(dead_code)]
    pub fn artists(&self) -> &Option<Vec<String>> {
        &self.artists
    }

    pub fn file_path(&self) -> &PathBuf {
        &self.file_path
    }

    #[allow(dead_code)]
    pub fn length(&self) -> i64 {
        self.length
    }
}

#[derive(Debug)]
pub enum Event {
    PlayerShutDown,
    PlaybackStatusChange(PlaybackStatus),
    Seeked { position: Duration },
    MetadataChange(Option<Metadata>),
}

#[derive(Debug)]
pub struct Progress {
    /// If player is stopped, metadata will be None
    metadata: Option<Metadata>,

    playback_status: PlaybackStatus,

    /// When this Progress was constructed, in order to calculate how old it is.
    instant: Instant,

    /// Position at the time of construction
    position: Duration,
}

impl Progress {
    pub fn new(
        playback_status: PlaybackStatus,
        position: Duration,
        metadata: Option<Metadata>,
    ) -> Progress {
        Progress {
            metadata,
            playback_status,
            instant: Instant::now(),
            position,
        }
    }

    pub fn metadata(&self) -> &Option<Metadata> {
        &self.metadata
    }

    pub fn take_metadata(self) -> Option<Metadata> {
        self.metadata
    }

    pub fn playback_status(&self) -> PlaybackStatus {
        self.playback_status
    }

    pub fn instant(&self) -> Instant {
        self.instant
    }

    pub fn position(&self) -> Duration {
        self.position
    }
}

fn query_player_property<T>(p: &ConnectionProxy, name: &str) -> Result<T, String>
where
    for<'b> T: dbus::arg::Get<'b>,
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

fn parse_player_metadata<T: arg::RefArg>(
    metadata_map: HashMap<String, T>,
) -> Result<Option<Metadata>, String> {
    debug!("metadata_map = {:?}", metadata_map);

    let file_path_encoded = match metadata_map.get("xesam:url") {
        Some(url) => url
            .as_str()
            .ok_or("url metadata should be string")?
            .to_string(),

        // If playlist has reached end, new metadata event is sent,
        // but it doesn't contain any of the following keys
        None => return Ok(None),
    };

    let file_path_url = Url::parse(&file_path_encoded)
        .map_err(|e| format!("invalid format of url metadata: {}", e.to_string()))?;
    let file_path = file_path_url
        .to_file_path()
        .map_err(|_| format!("invalid format of url metadata: {}", file_path_url))?;
    let album = metadata_map
        .get("xesam:album")
        .map(|v| {
            v.as_str()
                .ok_or("album metadata should be string")
                .map(|x| x.to_string())
        })
        .transpose()?;
    let title = metadata_map["xesam:title"]
        .as_str()
        .ok_or("title metadata should be string")?
        .to_string();
    let length = metadata_map["mpris:length"]
        .as_i64()
        .ok_or("length metadata should be i64")?;
    let artists = metadata_map
        .get("xesam:artist")
        .map(|v| {
            v.as_iter()
                .ok_or("artist metadata should be iterator")?
                .next()
                .ok_or("artist metadata should contain at least one entry")?
                .as_iter()
                .ok_or("artist metadata should have nested iterator")?
                .map(|x| {
                    Ok(x.as_str()
                        .ok_or("artist metadata values should be string")?
                        .to_string())
                })
                .collect::<Result<Vec<String>, &'static str>>()
        })
        .transpose()?;

    Ok(Some(Metadata {
        album,
        title,
        artists,
        file_path,
        length,
    }))
}

fn query_player_metadata(p: &ConnectionProxy) -> Result<Option<Metadata>, String> {
    query_player_property::<DbusStringMap>(p, "Metadata").and_then(parse_player_metadata)
}

pub fn query_progress(p: &ConnectionProxy) -> Result<Progress, String> {
    let playback_status = query_player_playback_status(p)?;
    let position = query_player_position(p)?;
    let instant = Instant::now();
    let metadata = if playback_status != PlaybackStatus::Stopped {
        query_player_metadata(p)?
    } else {
        None
    };
    Ok(Progress {
        metadata,
        playback_status,
        instant,
        position,
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

fn query_unique_owner_name<S: Into<String>>(c: &Connection, bus_name: S) -> Result<String, String> {
    let get_name_owner = Message::new_method_call(
        "org.freedesktop.DBus",
        "/",
        "org.freedesktop.DBus",
        "GetNameOwner",
    )
    .map_err(|e| e.to_string())?
    .append1(bus_name.into());

    c.send_with_reply_and_block(get_name_owner, Duration::from_millis(100))
        .map_err(|e| e.to_string())
        .map(|reply| {
            reply
                .get1()
                .expect("GetNameOwner must have name as first member")
        })
}

fn query_all_player_buses(c: &Connection) -> Result<Vec<String>, String> {
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

fn get_message_item_dict(
    a: &arg::Variant<Box<dyn arg::RefArg>>,
) -> HashMap<String, Box<&dyn arg::RefArg>> {
    let mut it = a.as_iter().unwrap();
    let d_variant = it.next().unwrap();
    let d_it = d_variant.as_iter().unwrap();
    let v = d_it.collect::<Vec<_>>();
    v.chunks(2)
        .map(|c| {
            let key = c[0].as_str().unwrap();
            (key.to_string(), Box::new(c[1]))
        })
        .collect()
}

#[derive(Debug)]
pub struct DbusPropertiesChangedHappened {
    pub interface_name: String,
    pub changed_properties: DbusStringMap,
    pub invalidated_properties: Vec<String>,
}

impl dbus::message::SignalArgs for DbusPropertiesChangedHappened {
    const NAME: &'static str = "PropertiesChanged";
    const INTERFACE: &'static str = "org.freedesktop.DBus.Properties";
}

impl arg::ReadAll for DbusPropertiesChangedHappened {
    fn read(i: &mut arg::Iter) -> Result<Self, arg::TypeMismatchError> {
        Ok(Self {
            interface_name: i.read()?,
            changed_properties: i.read()?,
            invalidated_properties: i.read()?,
        })
    }
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
    c: &'a Connection,
    player_owner_name: &'a str,
) -> ConnectionProxy<'a> {
    c.with_proxy(player_owner_name, MPRIS2_PATH, Duration::from_millis(5000))
}

fn get_mediaplayer2_seeked_handler(
    sender: Sender<Event>,
) -> impl Fn(MediaPlayer2SeekedHappened, &Connection) -> bool {
    move |e: MediaPlayer2SeekedHappened, _: &Connection| {
        debug!("Seek happened: {:?}", e);
        if e.position_us < 0 {
            panic!(
                "Position value must be positive number, found {}",
                e.position_us
            );
        }
        sender
            .send(Event::Seeked {
                position: Duration::from_micros(e.position_us as u64),
            })
            .unwrap();
        true
    }
}

fn get_dbus_properties_changed_handler(
    sender: Sender<Event>,
) -> impl Fn(DbusPropertiesChangedHappened, &Connection) -> bool {
    move |e: DbusPropertiesChangedHappened, _: &Connection| {
        debug!("DBus.Properties happened: {:?}", e);
        if e.interface_name == "org.mpris.MediaPlayer2.Player" {
            for (k, v) in &e.changed_properties {
                match k.as_ref() {
                    "PlaybackStatus" => {
                        let playback_status = v.as_str().unwrap();
                        debug!("playback_status = {:?}", playback_status);
                        sender
                            .send(Event::PlaybackStatusChange(parse_playback_status(
                                &playback_status,
                            )))
                            .unwrap();
                    }
                    "Metadata" => {
                        let metadata_map = get_message_item_dict(v);
                        debug!("metadata_map = {:?}", metadata_map);
                        let metadata = parse_player_metadata(metadata_map).unwrap();
                        sender.send(Event::MetadataChange(metadata)).unwrap();
                    }
                    _ => {
                        warn!("Unknown PropertiesChanged event:");
                        for p in &e.changed_properties {
                            warn!("    changed_property = {:?}", p);
                        }
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

fn get_dbus_name_owned_changed_handler(
    sender: Sender<Event>,
    player_owner_name: String,
) -> impl Fn(DbusNameOwnedChanged, &Connection) -> bool {
    move |e: DbusNameOwnedChanged, _: &Connection| {
        debug!("DbusNameOwnedChanged happened: {:?}", e);
        if e.name == player_owner_name && e.old_owner.is_empty() && e.new_owner == player_owner_name
        {
            sender.send(Event::PlayerShutDown).unwrap();
        }
        true
    }
}

pub fn subscribe<'a>(
    c: &'a Connection,
    player: &str,
    sender: &Sender<Event>,
) -> Result<String, String> {
    let all_player_buses = query_all_player_buses(&c)?;

    let player_bus = format!("{}{}", MPRIS2_PREFIX, player);
    if !all_player_buses.contains(&player_bus) {
        info!("all players = {:?}", all_player_buses);
        return Err("Player not running".to_owned());
    }

    let player_owner_name = query_unique_owner_name(&c, player_bus)?;
    debug!("player_owner_name = {:?}", player_owner_name);

    let p = get_connection_proxy(c, &player_owner_name);

    p.match_signal(get_dbus_properties_changed_handler(sender.clone()))
        .map_err(|e| e.to_string())?;

    p.match_signal(get_mediaplayer2_seeked_handler(sender.clone()))
        .map_err(|e| e.to_string())?;

    // p.match_signal(|_: MediaPlayer2TrackListChangeHappened, _: &Connection| {
    //     debug!("TrackList happened");
    //     true
    // }).map_err(|e| e.to_string())?;

    let proxy_generic_dbus = c.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_millis(5000),
    );
    proxy_generic_dbus
        .match_signal(get_dbus_name_owned_changed_handler(
            sender.clone(),
            player_owner_name.clone(),
        ))
        .map_err(|e| e.to_string())?;
    Ok(player_owner_name)
}
