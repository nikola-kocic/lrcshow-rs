use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dbus::stdintf::org_freedesktop_dbus::Properties;
use dbus::{arg, Connection, ConnectionItem, Message, MessageItem, MessageType};
use url::Url;

const MPRIS2_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS2_PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Metadata {
    album: String,
    title: String,
    artists: Vec<String>,
    file_path: PathBuf,
    length: i64,
}

impl Metadata {
    #[allow(dead_code)]
    pub fn album(&self) -> &String {
        &self.album
    }

    #[allow(dead_code)]
    pub fn title(&self) -> &String {
        &self.title
    }

    #[allow(dead_code)]
    pub fn artists(&self) -> &Vec<String> {
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
    MetadataChange(Metadata),
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
        metadata: Option<Metadata>,
        playback_status: PlaybackStatus,
        position: Duration,
    ) -> Progress {
        Progress {
            metadata,
            playback_status,
            instant: Instant::now(),
            position,
        }
    }

    pub fn metadata(&self) -> Option<Metadata> {
        self.metadata.clone()
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

fn unchecked_get_string(v: &arg::Variant<MessageItem>) -> &String {
    if let arg::Variant(MessageItem::Str(s)) = v {
        s
    } else {
        panic!("");
    }
}

fn query_player_property<T>(c: &Connection, dest: &str, name: &str) -> T
where
    for<'b> T: dbus::arg::Get<'b>,
{
    let p = c.with_path(dest, MPRIS2_PATH, 5000);
    p.get("org.mpris.MediaPlayer2.Player", name).unwrap()
}

pub fn query_player_position(c: &Connection, dest: &str) -> Duration {
    let v: i64 = query_player_property(c, dest, "Position");
    if v < 0 {
        panic!("Wrong position value");
    }
    Duration::from_micros(v as u64)
}

fn query_player_playback_status(c: &Connection, dest: &str) -> PlaybackStatus {
    let v: String = query_player_property(c, dest, "PlaybackStatus");
    parse_playback_status(&v)
}

fn parse_player_metadata(metadata_map: HashMap<String, arg::Variant<MessageItem>>) -> Metadata {
    // eprintln!("metadata_map = {:?}", metadata_map);
    let album = unchecked_get_string(&metadata_map["xesam:album"]).clone();
    let title = unchecked_get_string(&metadata_map["xesam:title"]).clone();
    let file_path_encoded = unchecked_get_string(&metadata_map["xesam:url"]).clone();
    let file_path_url = Url::parse(&file_path_encoded).unwrap();
    let file_path = file_path_url.to_file_path().unwrap();
    let length = if let arg::Variant(MessageItem::Int64(v)) = &metadata_map["mpris:length"] {
        *v
    } else {
        panic!("");
    };
    let artists: Vec<String> =
        if let arg::Variant(MessageItem::Array(a)) = &metadata_map["xesam:artist"] {
            a.iter().map(|e| {
                if let MessageItem::Str(s) = e {
                    s.clone()
                } else {
                    panic!("");
                }
            })
        } else {
            panic!("");
        }
        .collect();

    Metadata {
        album,
        title,
        artists,
        file_path,
        length,
    }
}

fn query_player_metadata(c: &Connection, dest: &str) -> Metadata {
    let metadata_map: HashMap<String, arg::Variant<MessageItem>> =
        query_player_property(c, dest, "Metadata");
    parse_player_metadata(metadata_map)
}

pub fn query_progress(c: &Connection, player_owner_name: &str) -> Progress {
    let playback_status = query_player_playback_status(c, player_owner_name);
    let position = query_player_position(c, player_owner_name);
    let instant = Instant::now();
    let metadata = if playback_status != PlaybackStatus::Stopped {
        Some(query_player_metadata(c, player_owner_name))
    } else {
        None
    };
    Progress {
        metadata,
        playback_status,
        instant,
        position,
    }
}

fn parse_playback_status(playback_status: &str) -> PlaybackStatus {
    match playback_status {
        "Playing" => PlaybackStatus::Playing,
        "Paused" => PlaybackStatus::Paused,
        "Stopped" => PlaybackStatus::Stopped,
        _ => panic!(""),
    }
}

fn query_unique_owner_name<S: Into<String>>(c: &Connection, bus_name: S) -> Option<String> {
    let get_name_owner = Message::new_method_call(
        "org.freedesktop.DBus",
        "/",
        "org.freedesktop.DBus",
        "GetNameOwner",
    )
    .unwrap()
    .append1(bus_name.into());

    c.send_with_reply_and_block(get_name_owner, 100)
        .ok()
        .and_then(|reply| reply.get1())
}

fn query_all_player_buses(c: &Connection) -> Result<Vec<String>, dbus::Error> {
    let list_names = Message::new_method_call(
        "org.freedesktop.DBus",
        "/",
        "org.freedesktop.DBus",
        "ListNames",
    )
    .unwrap();

    let reply = c.send_with_reply_and_block(list_names, 500)?;

    let names: arg::Array<&str, _> = reply.read1()?;

    Ok(names
        .filter(|name| name.starts_with(MPRIS2_PREFIX))
        .map(|str_ref| str_ref.to_owned())
        .collect())
}

fn get_properties_changed(
    m: &Message,
) -> (
    String,
    HashMap<String, arg::Variant<MessageItem>>,
    Vec<String>,
) {
    // STRING interface_name,
    // DICT<STRING,VARIANT> changed_properties,
    // ARRAY<STRING> invalidated_properties

    let mut iter = m.iter_init();
    let interface_name: String = iter.get().unwrap();
    // eprintln!("interface_name = {:?}", interface_name);
    iter.next();
    let changed_properties: HashMap<String, arg::Variant<MessageItem>> = iter.get().unwrap();
    iter.next();
    let invalidated_properties: Vec<String> = iter.get().unwrap();

    (interface_name, changed_properties, invalidated_properties)
}

fn try_parse_name_owner_changed(message: &Message) -> Option<(String, String)> {
    match (message.sender(), message.member()) {
        (Some(ref sender), Some(ref member))
            if &**sender == "org.freedesktop.DBus" && &**member == "NameOwnerChanged" =>
        {
            let mut iter = message.iter_init();
            let name: String = iter.read().ok()?;

            if !name.starts_with("org.mpris.") {
                None
            } else {
                let old_owner: String = iter.read().ok()?;
                let new_owner: String = iter.read().ok()?;
                Some((new_owner, old_owner))
            }
        }
        _ => None,
    }
}

fn get_message_item_dict(a: &dbus::MessageItemArray) -> HashMap<String, arg::Variant<MessageItem>> {
    a.iter()
        .map(|e| {
            if let MessageItem::DictEntry(box_str, box_var) = e.clone() {
                if let (MessageItem::Str(s), MessageItem::Variant(v)) = (*box_str, *box_var) {
                    (s, arg::Variant(*v))
                } else {
                    panic!("");
                }
            } else {
                panic!("");
            }
        })
        .collect()
}

pub fn create_events(ci: &ConnectionItem, player_owner_name: &str) -> Vec<Event> {
    let mut events = Vec::new();

    let m = if let ConnectionItem::Signal(ref s) = *ci {
        s
    } else {
        return events;
    };

    if let Some((new_owner, old_owner)) = try_parse_name_owner_changed(m) {
        if new_owner.is_empty() && old_owner == player_owner_name {
            events.push(Event::PlayerShutDown);
            return events;
        }
    }

    let (msg_type, msg_path, msg_interface, msg_member) = m.headers();
    if msg_type != MessageType::Signal {
        return events;
    };

    let msg_path = msg_path.unwrap();

    if msg_path != MPRIS2_PATH {
        return events;
    }

    // eprintln!("{:?}", m);
    // let unique_name = m.sender().map(|bus_name| bus_name.to_string());
    // eprintln!("Sender: {:?}", unique_name);

    let msg_interface = msg_interface.unwrap();
    let msg_member = msg_member.unwrap();

    match msg_interface.as_ref() {
        "org.mpris.MediaPlayer2.Player" => {
            if let "Seeked" = msg_member.as_ref() {
                let v = m.get1::<i64>().unwrap();
                if v < 0 {
                    panic!("");
                }
                events.push(Event::Seeked {
                    position: Duration::from_micros(v as u64),
                });
            }
        }
        "org.freedesktop.DBus.Properties" => {
            if let "PropertiesChanged" = msg_member.as_ref() {
                // eprintln!("PropertiesChanged");
                let (interface_name, changed_properties, invalidated_properties) =
                    get_properties_changed(&m);
                if interface_name == "org.mpris.MediaPlayer2.Player" {
                    for (k, v) in &changed_properties {
                        match k.as_ref() {
                            "PlaybackStatus" => {
                                let playback_status = unchecked_get_string(v);
                                events.push(Event::PlaybackStatusChange(parse_playback_status(
                                    &playback_status,
                                )));
                            }
                            "Metadata" => {
                                let metadata_map = if let arg::Variant(MessageItem::Array(a)) = v {
                                    get_message_item_dict(a)
                                } else {
                                    panic!("");
                                };
                                let metadata = parse_player_metadata(metadata_map);
                                events.push(Event::MetadataChange(metadata));
                            }
                            _ => {
                                eprintln!("Unknown PropertiesChanged event:");
                                for p in &changed_properties {
                                    eprintln!("    changed_property = {:?}", p);
                                }
                                eprintln!(
                                    "    invalidated_properties = {:?}",
                                    invalidated_properties
                                );
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    events
}

pub fn subscribe(c: &Connection, player: &str) -> Option<String> {
    let all_player_buses = query_all_player_buses(&c).unwrap();

    let player_bus = format!("{}{}", MPRIS2_PREFIX, player);
    if !all_player_buses.contains(&player_bus) {
        eprintln!("Player not running");
        eprintln!("all players = {:?}", all_player_buses);
        return None;
    }

    let player_owner_name = query_unique_owner_name(&c, player_bus).unwrap();
    eprintln!("player_owner_name = {:?}", player_owner_name);

    c.add_match(&format!("interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',path='/org/mpris/MediaPlayer2',sender='{}'", player_owner_name)).unwrap();

    c.add_match(
        &format!("interface='org.mpris.MediaPlayer2.Player',member='Seeked',path='/org/mpris/MediaPlayer2',sender='{}'", player_owner_name)
    )
    .unwrap();

    c.add_match(&format!(
        "interface='org.mpris.MediaPlayer2.TrackList',path='/org/mpris/MediaPlayer2',sender='{}'",
        player_owner_name
    ))
    .unwrap();

    c.add_match("type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged'").unwrap();
    Some(player_owner_name)
}
