use std::collections::HashMap;
use std::convert::TryFrom;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::{AppendAll, RefArg};
use dbus::blocking::stdintf::org_freedesktop_dbus::{Properties, PropertiesPropertiesChanged};
use dbus::blocking::BlockingSender;
use dbus::blocking::{LocalConnection, Proxy};
use dbus::message::SignalArgs;
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

pub type BusName = dbus::strings::BusName<'static>;

pub fn get_connection_proxy(
    c: &LocalConnection,
    player_owner_name: BusName,
) -> Proxy<&LocalConnection> {
    debug!("get_connection_proxy with {player_owner_name}");
    c.with_proxy(player_owner_name, MPRIS2_PATH, Duration::from_millis(5000))
}

pub struct QueryPlayerProperties<'a, C: BlockingSender> {
    pub proxy: Proxy<'a, &'a C>,
}

impl<'a, C: BlockingSender> QueryPlayerProperties<'a, C> {
    fn query_player_property(&self, name: &str) -> Result<Box<dyn RefArg>, String> {
        self.proxy
            .get("org.mpris.MediaPlayer2.Player", name)
            .map_err(|e| e.to_string())
    }

    pub fn query_player_position(&self) -> Result<Duration, String> {
        let v = self.query_player_property("Position")?;
        parse_player_position(&v)
    }

    pub fn query_player_state(&self) -> Result<PlayerState, String> {
        let properties = self
            .proxy
            .get_all("org.mpris.MediaPlayer2.Player")
            .map_err(|e| e.to_string())?;
        parse_player_state(&properties, Instant::now())
    }
}

fn parse_player_position(arg: &dyn RefArg) -> Result<Duration, String> {
    let v = arg
        .as_i64()
        .ok_or(format!("Position should be an i64 value, got {arg:?}"))?;
    let micros = u64::try_from(v).map_err(|e| format!("Wrong position value: {v}, {e}"))?;
    Ok(Duration::from_micros(micros))
}

fn parse_player_playback_status(playback_status: &dyn RefArg) -> Result<PlaybackStatus, String> {
    playback_status
        .as_str()
        .ok_or(format!(
            "PlaybackStatus should be a string, got {playback_status:?}"
        ))
        .map(parse_playback_status)
}

fn parse_player_metadata(
    metadata_variant: &dbus::arg::Variant<Box<dyn RefArg>>,
) -> Result<Option<Metadata>, String> {
    let mut file_path_uri: Option<&str> = None;
    debug!("parse_player_metadata");

    let mut metadata_iter = metadata_variant
        .0
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

fn try_get_value<'a, V>(
    hash_map: &'a HashMap<String, V>,
    key: &'static str,
) -> Result<&'a V, String> {
    hash_map.get(key).ok_or(format!("Missing {key}"))
}

fn parse_player_state(
    properties: &arg::PropMap,
    current_instant: Instant,
) -> Result<PlayerState, String> {
    debug!("parse_player_state: {properties:?}");
    let playback_status =
        parse_player_playback_status(try_get_value(properties, "PlaybackStatus")?)?;
    let position = parse_player_position(try_get_value(properties, "Position")?)?;
    let metadata = if playback_status == PlaybackStatus::Stopped {
        None
    } else {
        let m = try_get_value(properties, "Metadata")?;
        parse_player_metadata(m)?
    };
    Ok(PlayerState {
        playback_status,
        position_snapshot: PositionSnapshot {
            position,
            instant: current_instant,
        },
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

#[derive(Debug)]
struct MediaPlayer2SeekedHappened {
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
struct DBusNameOwnerChanged {
    pub name: String,
    pub new_owner: String,
    pub old_owner: String,
}

impl arg::AppendAll for DBusNameOwnerChanged {
    fn append(&self, i: &mut arg::IterAppend) {
        arg::RefArg::append(&self.name, i);
        arg::RefArg::append(&self.new_owner, i);
        arg::RefArg::append(&self.old_owner, i);
    }
}

impl dbus::message::SignalArgs for DBusNameOwnerChanged {
    const NAME: &'static str = "NameOwnerChanged";
    const INTERFACE: &'static str = "org.freedesktop.DBus";
}

impl arg::ReadAll for DBusNameOwnerChanged {
    fn read(i: &mut arg::Iter) -> Result<Self, arg::TypeMismatchError> {
        Ok(Self {
            name: i.read()?,
            new_owner: i.read()?,
            old_owner: i.read()?,
        })
    }
}

fn react_on_changed_seek_value<F: FnMut(PlayerEvent)>(e: &MediaPlayer2SeekedHappened, mut f: F) {
    debug!("Seek happened: {:?}", e);
    let position = Duration::from_micros(u64::try_from(e.position_us).unwrap());
    f(PlayerEvent::Seeked { position });
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
                f(PlayerEvent::PlaybackStatusChange(playback_status));
            }
            "Metadata" => {
                let metadata = parse_player_metadata(&v).unwrap();
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
                    value: format!("{v:?}"),
                });
            }
        }
    }
}

struct PlayerBusOwnerNameFinder<'a> {
    connection: &'a dyn BlockingSender,
    player_bus: &'a String,
}

impl<'a> PlayerBusOwnerNameFinder<'a> {
    fn query_all_player_buses(&self) -> Result<Vec<String>, String> {
        let list_names = Message::new_method_call(
            "org.freedesktop.DBus",
            "/",
            "org.freedesktop.DBus",
            "ListNames",
        )?;

        let reply = self
            .connection
            .send_with_reply_and_block(list_names, Duration::from_millis(500))
            .map_err(|e| e.to_string())?;

        let names: arg::Array<&str, _> = reply.read1().map_err(|e| e.to_string())?;

        Ok(names
            .filter_map(|name| {
                if name.starts_with(MPRIS2_PREFIX) {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
            .collect())
    }

    fn query_unique_owner_name(&self) -> Result<BusName, String> {
        let get_name_owner = Message::new_method_call(
            "org.freedesktop.DBus",
            "/",
            "org.freedesktop.DBus",
            "GetNameOwner",
        )?
        .append1(self.player_bus);

        let unique_owner_name: String = self
            .connection
            .send_with_reply_and_block(get_name_owner, Duration::from_millis(100))
            .map_err(|e| e.to_string())
            .map(|reply| {
                reply
                    .get1::<String>()
                    .expect("GetNameOwner must have name as first member")
            })?;
        BusName::new(unique_owner_name)
    }

    fn query_player_owner_name(&self) -> Result<Option<BusName>, String> {
        let all_player_buses = self.query_all_player_buses()?;

        if !all_player_buses.contains(self.player_bus) {
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

        let player_owner_name = self.query_unique_owner_name()?;
        debug!("player_owner_name = {:?}", player_owner_name);
        Ok(Some(player_owner_name))
    }
}

#[derive(Debug)]
enum DbusPlayerEvent {
    PropertiesChanged(PropertiesPropertiesChanged),
    Seek(MediaPlayer2SeekedHappened),
    DBusNameOwnerChanged(DBusNameOwnerChanged),
}

type TimedPlayerDbusEvent = crate::events::TimedEventBase<DbusPlayerEvent>;

pub struct PlayerNotifications<'a> {
    connection: &'a LocalConnection,
    sender: Sender<TimedEvent>,
    proxy_generic_dbus: Proxy<'a, &'a LocalConnection>,
    dbus_event_sender: Sender<TimedPlayerDbusEvent>,
    dbus_event_receiver: Receiver<TimedPlayerDbusEvent>,
    player_bus: String,
}

impl<'a> PlayerNotifications<'a> {
    fn new(connection: &'a LocalConnection, sender: Sender<TimedEvent>, player: &str) -> Self {
        let proxy_generic_dbus = connection.with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_millis(5000),
        );
        let (dbus_event_sender, dbus_event_receiver) = channel::<TimedPlayerDbusEvent>();

        let player_bus = format!("{MPRIS2_PREFIX}{player}");
        PlayerNotifications {
            connection,
            sender,
            proxy_generic_dbus,
            dbus_event_sender,
            dbus_event_receiver,
            player_bus,
        }
    }

    fn create_dbus_handler<T>(
        &self,
        constructor: impl Fn(T) -> DbusPlayerEvent,
    ) -> impl Fn(T, &LocalConnection, &Message) -> bool {
        let tx = self.dbus_event_sender.clone();
        move |e: T, _: &LocalConnection, _: &Message| {
            tx.send(TimedPlayerDbusEvent {
                instant: Instant::now(),
                event: constructor(e),
            })
            .unwrap();
            true
        }
    }

    fn subscribe(
        &self,
        dbus_proxy_player: &Proxy<'a, &'a LocalConnection>,
    ) -> Result<(), dbus::Error> {
        dbus_proxy_player
            .match_signal(self.create_dbus_handler(DbusPlayerEvent::PropertiesChanged))?;

        dbus_proxy_player.match_signal(self.create_dbus_handler(DbusPlayerEvent::Seek))?;

        // dbus_proxy_player.match_signal(|_: MediaPlayer2TrackListChangeHappened, _: &Connection, _: &Message| {
        //     debug!("TrackList happened");
        //     true
        // })?;

        Ok(())
    }

    fn react_on_dbus_name_owned_changed<F: FnMut(PlayerEvent)>(
        &self,
        e: DBusNameOwnerChanged,
        dbus_proxy_player: &mut Option<Proxy<'a, &'a LocalConnection>>,
        mut f: F,
    ) {
        if e.name == self.player_bus {
            if e.old_owner.is_empty() {
                *dbus_proxy_player = None;
                f(PlayerEvent::PlayerShutDown)
            } else if e.new_owner.is_empty() {
                let player_owner_name = BusName::new(e.name).unwrap();
                *dbus_proxy_player = Some(get_connection_proxy(
                    self.connection,
                    player_owner_name.clone(),
                ));
                self.subscribe(dbus_proxy_player.as_ref().unwrap())
                    .map_err(|e| e.to_string())
                    .unwrap();
                f(PlayerEvent::PlayerStarted { player_owner_name })
            }
        }
    }

    fn on_dbus_event<F: FnMut(PlayerEvent)>(
        &self,
        dbus_event: DbusPlayerEvent,
        dbus_proxy_player: &mut Option<Proxy<'a, &'a LocalConnection>>,
        f: F,
    ) {
        debug!("on_dbus_event: {dbus_event:?}");
        match dbus_event {
            DbusPlayerEvent::PropertiesChanged(e) => {
                if e.interface_name == "org.mpris.MediaPlayer2.Player" {
                    react_on_changed_properties(e.changed_properties, f)
                }
            }
            DbusPlayerEvent::Seek(e) => react_on_changed_seek_value(&e, f),
            DbusPlayerEvent::DBusNameOwnerChanged(e) => {
                self.react_on_dbus_name_owned_changed(e, dbus_proxy_player, f)
            }
        }
    }

    fn initial_try_connect_to_player(&self) {
        let player_owner_bus_finder = PlayerBusOwnerNameFinder {
            connection: self.connection,
            player_bus: &self.player_bus,
        };
        if let Some(o) = player_owner_bus_finder.query_player_owner_name().unwrap() {
            let mut msg = dbus::Message::new_signal(
                self.proxy_generic_dbus.path.to_string(),
                DBusNameOwnerChanged::INTERFACE,
                DBusNameOwnerChanged::NAME,
            )
            .unwrap();
            let data = DBusNameOwnerChanged {
                name: o.to_string(),
                new_owner: "".to_string(),
                old_owner: o.to_string(),
            };
            let mut m = dbus::arg::IterAppend::new(&mut msg);
            data.append(&mut m);
            let handler = self.create_dbus_handler(DbusPlayerEvent::DBusNameOwnerChanged);
            handler(data, self.connection, &msg);
        }
    }

    fn run_sync(&self) -> Result<(), dbus::Error> {
        let mut dbus_proxy_player: Option<Proxy<'a, &'a LocalConnection>> = None;

        let dbus_name_owner_changed_token = self
            .proxy_generic_dbus
            .match_signal(self.create_dbus_handler(DbusPlayerEvent::DBusNameOwnerChanged))
            .unwrap();

        self.initial_try_connect_to_player();

        'outer: loop {
            'inner: loop {
                match self.dbus_event_receiver.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => break 'inner,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'outer,
                    Ok(dbus_event) => {
                        let instant = dbus_event.instant;
                        self.on_dbus_event(
                            dbus_event.event,
                            &mut dbus_proxy_player,
                            |player_event| {
                                self.sender
                                    .send(TimedEvent {
                                        instant,
                                        event: Event::PlayerEvent(player_event),
                                    })
                                    .unwrap();
                            },
                        );
                    }
                }
            }
            self.connection.process(Duration::from_millis(16))?;
        }
        self.proxy_generic_dbus
            .match_stop(dbus_name_owner_changed_token, true)?;
        Ok(())
    }

    pub fn run_async(player: String, sender: Sender<TimedEvent>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let connection = LocalConnection::new_session().unwrap();
            let o = PlayerNotifications::new(&connection, sender, &player);
            o.run_sync().unwrap();
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use dbus::arg::Variant;
    use test_log::test;

    fn create_test_metadata_1() -> dbus::arg::PropMap {
        let mut metadata = dbus::arg::PropMap::new();
        metadata.insert(
            "mpris:artUrl".to_owned(),
            Variant(Box::new("file:///tmp/audacious-temp-75WRR2".to_string())),
        );
        metadata.insert(
            "mpris:trackid".to_owned(),
            Variant(Box::new(
                dbus::strings::Path::from_slice("/org/mpris/MediaPlayer2/CurrentTrack\0").unwrap(),
            )),
        );
        metadata.insert(
            "xesam:artist".to_owned(),
            Variant(Box::new(vec!["Queen".to_owned()])),
        );
        metadata.insert(
            "xesam:album".to_owned(),
            Variant(Box::new("Greatest Hits II".to_owned())),
        );

        metadata.insert(
            "xesam:url".to_owned(),
            Variant(Box::new(
                "file:///home/user/music/Queen/--%20Compilations%20--/%281991%29%20Greatest%20Hits%20II/13%20Queen%20-%20The%20Invisible%20Man.mp3".to_owned()
            ))
        );
        metadata.insert("mpris:length".to_owned(), Variant(Box::new(238655000)));

        metadata.insert(
            "xesam:title".to_owned(),
            Variant(Box::new("The Invisible Man".to_owned())),
        );

        metadata
    }

    #[test]
    fn test_react_on_changed_properties_regular() {
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
        //            "Metadata": Variant({
        //                "mpris:artUrl": Variant("file:///tmp/audacious-temp-75WRR2"),
        //                "mpris:trackid": Variant(Path("/org/mpris/MediaPlayer2/CurrentTrack\0")),
        //                "xesam:artist": Variant(["Queen"]),
        //                "xesam:album": Variant("Greatest Hits II"),
        //                "xesam:url": Variant("file:///home/user/music/Queen/--%20Compilations%20--/%281991%29%20Greatest%20Hits%20II/13%20Queen%20-%20The%20Invisible%20Man.mp3"),
        //                "mpris:length": Variant(238655000),
        //                "xesam:title": Variant("The Invisible Man")
        //            })
        //         },
        //         []
        //     ]
        // }

        let mut changed_properties = dbus::arg::PropMap::new();
        changed_properties.insert(
            "Metadata".to_owned(),
            Variant(Box::new(create_test_metadata_1())),
        );

        let mut reported_events: Vec<PlayerEvent> = Vec::<PlayerEvent>::new();
        react_on_changed_properties(changed_properties, |player_event| {
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

    #[test]
    fn test_parse_state() {
        // {
        //     "CanPlay": Variant(true),
        //     "CanPause": Variant(true),
        //     "CanSeek": Variant(true),
        //     "PlaybackStatus": Variant("Paused"),
        //     "CanGoNext": Variant(true),
        //     "CanControl": Variant(true),
        //     "Position": Variant(41337000),
        //     "Volume": Variant(0.5),
        //     "CanGoPrevious": Variant(true),
        //     "Metadata": Variant({
        //         "mpris:length": Variant(238655000),
        //         "xesam:album": Variant("Greatest Hits II"),
        //         "xesam:url": Variant("file:///home/nikola/Music/from-phone/Music/mnt/backup2/music/english/Queen/--%20Compilations%20--/%281991%29%20Greatest%20Hits%20II/13%20Queen%20-%20The%20Invisible%20Man.mp3"),
        //         "mpris:trackid": Variant(Path("/org/mpris/MediaPlayer2/CurrentTrack\0")),
        //         "mpris:artUrl": Variant("file:///tmp/audacious-temp-75WRR2"),
        //         "xesam:artist": Variant(["Queen"]),
        //         "xesam:title": Variant("The Invisible Man")
        //     })
        // }

        let mut properties = dbus::arg::PropMap::new();
        properties.insert("CanPlay".to_owned(), Variant(Box::new(true)));
        properties.insert("CanPause".to_owned(), Variant(Box::new(true)));
        properties.insert("CanSeek".to_owned(), Variant(Box::new(true)));
        properties.insert(
            "PlaybackStatus".to_owned(),
            Variant(Box::new("Paused".to_owned())),
        );
        properties.insert("CanGoNext".to_owned(), Variant(Box::new(true)));
        properties.insert("CanControl".to_owned(), Variant(Box::new(true)));
        properties.insert("Position".to_owned(), Variant(Box::new(41337000)));
        properties.insert("Volume".to_owned(), Variant(Box::new(0.5)));
        properties.insert("CanGoPrevious".to_owned(), Variant(Box::new(true)));
        properties.insert(
            "Metadata".to_owned(),
            Variant(Box::new(create_test_metadata_1())),
        );

        let instant = Instant::now();
        let state = parse_player_state(&properties, instant);
        assert_eq!(state, Ok(PlayerState {
            playback_status: PlaybackStatus::Paused,
            position_snapshot: PositionSnapshot{
                position: Duration::from_micros(41337000),
                instant,
            },
            metadata: Some(Metadata {
                file_path: PathBuf::from_str(
                    "/home/user/music/Queen/-- Compilations --/(1991) Greatest Hits II/13 Queen - The Invisible Man.mp3"
                ).unwrap()})
            }));
    }
}
