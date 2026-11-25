use std::{
    net::{IpAddr, Ipv4Addr},
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use rust_cast::{
    CastDevice, ChannelMessage,
    channels::{
        connection::ConnectionResponse,
        heartbeat::HeartbeatResponse,
        media::{IdleReason, LoadOptions, Media, PlayerState, StreamType},
        receiver::{CastDeviceApp, ReceiverResponse, Volume},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Device {
    pub name: String,
    pub ip: IpAddr,
    pub port: u16,
}

/// Browses mDNS for Cast devices for `wait`, blocking the caller.
pub fn discover(wait: Duration) -> anyhow::Result<Vec<Device>> {
    let mdns = ServiceDaemon::new()?;
    let events = mdns.browse("_googlecast._tcp.local.")?;
    let deadline = Instant::now() + wait;
    let mut found: Vec<Device> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(s) = event {
            let Some(ip) = s.get_addresses_v4().into_iter().next() else {
                continue;
            };
            let name = s
                .get_property_val_str("fn")
                .unwrap_or(&s.fullname)
                .to_string();
            if !found.iter().any(|d| d.name == name) {
                found.push(Device {
                    name,
                    ip: IpAddr::V4(ip),
                    port: s.port,
                });
            }
        }
    }
    let _ = mdns.shutdown();
    Ok(found)
}

/// Interface names that are tunnels or virtual links: a LAN device cannot
/// reach this machine through them.
const VIRTUAL: &[&str] = &[
    "utun",
    "tun",
    "tap",
    "wg",
    "ppp",
    "ipsec",
    "gif",
    "stf",
    "awdl",
    "llw",
    "anpi",
    "bridge",
    "vmnet",
    "vboxnet",
    "docker",
    "veth",
    "tailscale",
    "zt",
];

/// Picks the address LAN devices should use to reach this machine, from
/// `(interface name, address, netmask)` triples. The interface on the same
/// subnet as `target` wins (an IPv6 target matches none); otherwise the first
/// private address on a physical interface. Tunnels are never chosen, so an
/// active VPN does not leak in.
pub fn pick_lan_ip(
    ifaces: &[(&str, Ipv4Addr, Ipv4Addr)],
    target: Option<IpAddr>,
) -> Option<Ipv4Addr> {
    let target = match target {
        Some(IpAddr::V4(t)) => Some(t),
        _ => None,
    };
    let physical = ifaces
        .iter()
        .filter(|(name, ip, _)| !ip.is_loopback() && !VIRTUAL.iter().any(|v| name.starts_with(v)));
    let same_subnet = |&&(_, ip, mask): &&(&str, Ipv4Addr, Ipv4Addr)| {
        target.is_some_and(|t| u32::from(ip) & u32::from(mask) == u32::from(t) & u32::from(mask))
    };
    physical
        .clone()
        .find(same_subnet)
        .or_else(|| physical.clone().find(|(_, ip, _)| ip.is_private()))
        .map(|(_, ip, _)| *ip)
}

/// This machine's LAN address as seen from `target`, or loopback if none.
pub fn lan_ip(target: Option<IpAddr>) -> Ipv4Addr {
    let ifaces = if_addrs::get_if_addrs().unwrap_or_default();
    let v4: Vec<_> = ifaces
        .iter()
        .filter_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(a) => Some((i.name.as_str(), a.ip, a.netmask)),
            _ => None,
        })
        .collect();
    pick_lan_ip(&v4, target).unwrap_or(Ipv4Addr::LOCALHOST)
}

/// What to load on the device: a path on the stream server and how to play it.
#[derive(Debug, Clone)]
pub struct Load {
    pub path: String,
    pub content_type: String,
    pub start: f64,
    pub duration: Option<f64>,
}

/// Remote control commands for a running session.
#[derive(Debug)]
pub enum Command {
    Play,
    Pause,
    Seek(f64),
    /// Stream volume on the device, 0 to 1.
    Volume(f32),
    Mute(bool),
    /// Closes the app on the device, then signals the sender.
    Stop(mpsc::Sender<()>),
    /// Loads another URL in the same session, without relaunching the app.
    Load(String, Load, mpsc::Sender<Result<(), String>>),
}

/// Loads `url` in the running session behind `commands`; waits for the
/// device to accept it (up to 20 s).
pub fn reload(commands: &mpsc::Sender<Command>, url: String, load: Load) -> anyhow::Result<()> {
    let (done, loaded) = mpsc::channel();
    commands
        .send(Command::Load(url, load, done))
        .map_err(|_| anyhow::anyhow!("cast session is over"))?;
    loaded
        .recv_timeout(Duration::from_secs(20))?
        .map_err(anyhow::Error::msg)
}

fn media_of(url: String, load: &Load) -> (Media, LoadOptions) {
    let media = Media {
        content_id: url,
        stream_type: StreamType::Buffered,
        content_type: load.content_type.clone(),
        metadata: None,
        duration: load.duration.map(|d| d as f32),
    };
    (
        media,
        LoadOptions {
            current_time: load.start,
            autoplay: true,
        },
    )
}

/// Stops the session behind `commands` and waits (up to 3 s) until the
/// device has closed the app, so a new session never races the old one.
pub fn stop(commands: &mpsc::Sender<Command>) {
    let (done, closed) = mpsc::channel();
    if commands.send(Command::Stop(done)).is_ok() {
        let _ = closed.recv_timeout(Duration::from_secs(3));
    }
}

/// What a session reports back, about twice a second.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    Status {
        time: f64,
        duration: Option<f64>,
        state: &'static str,
    },
    Volume {
        level: f32,
        muted: bool,
    },
    /// The device stopped playing; `time` is the last known position.
    Ended {
        time: f64,
        finished: bool,
    },
}

/// Turns the media status of the session's item into an event. No item, or
/// an idle item with a reason, means the session is over.
pub fn interpret(
    entry: Option<(PlayerState, Option<IdleReason>, Option<f32>)>,
    last_time: f64,
    duration: Option<f64>,
) -> Event {
    let Some((state, idle, time)) = entry else {
        return Event::Ended {
            time: last_time,
            finished: false,
        };
    };
    let time = time.map_or(last_time, f64::from);
    match (state, idle) {
        (PlayerState::Idle, Some(reason)) => Event::Ended {
            time: last_time,
            finished: matches!(reason, IdleReason::Finished),
        },
        (PlayerState::Playing, _) => Event::Status {
            time,
            duration,
            state: "playing",
        },
        (PlayerState::Paused, _) => Event::Status {
            time,
            duration,
            state: "paused",
        },
        // Buffering, or idle while the item is still loading.
        _ => Event::Status {
            time,
            duration,
            state: "buffering",
        },
    }
}

fn volume(v: Volume) -> Event {
    Event::Volume {
        level: v.level.unwrap_or(1.0),
        muted: v.muted.unwrap_or(false),
    }
}

/// Starts the Default Media Receiver on the device, loads `base` + the media
/// path and keeps the session on a thread of its own: it runs `commands` and
/// reports status through `on_event` until the device stops, the receiver is
/// taken over, or the command sender is dropped (a newer session replaced
/// this one; then no `Ended` is sent). Returns once the media is loaded.
pub fn play(
    device: &Device,
    base: &str,
    load: Load,
    commands: mpsc::Receiver<Command>,
    on_event: impl Fn(Event) + Send + 'static,
) -> anyhow::Result<()> {
    let url = format!("{base}{}", load.path);
    let (host, port) = (device.ip.to_string(), device.port);
    let (done, result) = mpsc::channel();
    std::thread::spawn(move || {
        let session = (|| -> anyhow::Result<_> {
            // Cast devices present self-signed certificates.
            let cast = CastDevice::connect_without_host_verification(host, port)?;
            cast.connection.connect("receiver-0")?;
            let app = cast
                .receiver
                .launch_app(&CastDeviceApp::DefaultMediaReceiver)?;
            cast.connection.connect(app.transport_id.clone())?;
            let (media, opts) = media_of(url, &load);
            let status = cast.media.load_with_opts(
                app.transport_id.clone(),
                app.session_id.clone(),
                &media,
                opts,
            )?;
            let media_id = status
                .entries
                .first()
                .map(|e| e.media_session_id)
                .context("no media session")?;
            Ok((cast, app, media_id))
        })();
        let (cast, app, media_id) = match session {
            Ok(s) => {
                let _ = done.send(Ok(()));
                s
            }
            Err(e) => {
                let _ = done.send(Err(e));
                return;
            }
        };
        let (transport, session_id) = (&app.transport_id, &app.session_id);
        let event = run(
            &cast,
            transport,
            session_id,
            media_id,
            load.start,
            load.duration,
            &commands,
            &on_event,
        );
        if let Some(event) = event {
            on_event(event);
        }
    });
    result.recv()?
}

/// The session loop. Every tick runs queued commands, pings, asks for the
/// media status, then reads messages until the pong: the socket is shared
/// between reads and writes, so reads only wait for replies already asked
/// for. rust_cast sets no socket timeout, so a device that vanishes without
/// closing the connection leaves this thread waiting.
#[allow(clippy::too_many_arguments)]
fn run(
    cast: &CastDevice,
    transport: &str,
    session_id: &str,
    mut media_id: i32,
    mut last_time: f64,
    mut duration: Option<f64>,
    commands: &mpsc::Receiver<Command>,
    on_event: &dyn Fn(Event),
) -> Option<Event> {
    let ended = |time| {
        Some(Event::Ended {
            time,
            finished: false,
        })
    };
    if let Ok(s) = cast.receiver.get_status() {
        on_event(volume(s.volume));
    }
    loop {
        loop {
            let result = match commands.try_recv() {
                Ok(Command::Play) => cast.media.play(transport, media_id).map(drop),
                Ok(Command::Pause) => cast.media.pause(transport, media_id).map(drop),
                Ok(Command::Seek(t)) => cast
                    .media
                    .seek(transport, media_id, Some(t as f32), None)
                    .map(drop),
                Ok(Command::Volume(level)) => {
                    cast.receiver.set_volume(level).map(|v| on_event(volume(v)))
                }
                Ok(Command::Mute(muted)) => {
                    cast.receiver.set_volume(muted).map(|v| on_event(volume(v)))
                }
                Ok(Command::Load(url, next, done)) => {
                    let (media, opts) = media_of(url, &next);
                    let loaded = cast
                        .media
                        .load_with_opts(transport, session_id, &media, opts);
                    match loaded.map(|s| s.entries.first().map(|e| e.media_session_id)) {
                        Ok(Some(id)) => {
                            media_id = id;
                            last_time = next.start;
                            duration = next.duration;
                            let _ = done.send(Ok(()));
                            Ok(())
                        }
                        Ok(None) => {
                            let _ = done.send(Err("no media session".into()));
                            return ended(last_time);
                        }
                        Err(e) => {
                            let _ = done.send(Err(e.to_string()));
                            return ended(last_time);
                        }
                    }
                }
                Ok(Command::Stop(done)) => {
                    let _ = cast.receiver.stop_app(session_id);
                    let _ = done.send(());
                    return ended(last_time);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                // The app dropped the session without stopping it: leave the TV idle.
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = cast.receiver.stop_app(session_id);
                    return None;
                }
            };
            if result.is_err() {
                return ended(last_time);
            }
        }
        if cast.heartbeat.ping().is_err() {
            return ended(last_time);
        }
        let Ok(status) = cast.media.get_status(transport, Some(media_id)) else {
            return ended(last_time);
        };
        let entry = status
            .entries
            .iter()
            .find(|e| e.media_session_id == media_id)
            .map(|e| (e.player_state, e.idle_reason, e.current_time));
        match interpret(entry, last_time, duration) {
            Event::Status {
                time,
                duration,
                state,
            } => {
                last_time = time;
                // Reported below, after the pong, so a closed session wins.
                let status = Event::Status {
                    time,
                    duration,
                    state,
                };
                loop {
                    match cast.receive() {
                        Ok(ChannelMessage::Heartbeat(HeartbeatResponse::Pong)) => break,
                        Ok(ChannelMessage::Heartbeat(HeartbeatResponse::Ping)) => {
                            let _ = cast.heartbeat.pong();
                        }
                        Ok(ChannelMessage::Connection(ConnectionResponse::Close)) => {
                            return ended(last_time);
                        }
                        Ok(ChannelMessage::Receiver(ReceiverResponse::Status(s))) => {
                            if !s.applications.iter().any(|a| a.session_id == session_id) {
                                return ended(last_time);
                            }
                            // Volume changed elsewhere: phone, Google Home, TV remote.
                            on_event(volume(s.volume));
                        }
                        Ok(_) => {}
                        Err(_) => return ended(last_time),
                    }
                }
                on_event(status);
            }
            end => return Some(end),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    const MASK24: &str = "255.255.255.0";

    #[test]
    fn status_maps_player_states() {
        let d = Some(100.0);
        let status = |state| Event::Status {
            time: 12.5,
            duration: d,
            state,
        };
        assert_eq!(
            interpret(Some((PlayerState::Playing, None, Some(12.5))), 3.0, d),
            status("playing")
        );
        assert_eq!(
            interpret(Some((PlayerState::Paused, None, Some(12.5))), 3.0, d),
            status("paused")
        );
        assert_eq!(
            interpret(Some((PlayerState::Buffering, None, Some(12.5))), 3.0, d),
            status("buffering")
        );
        // Idle without a reason: still loading.
        assert_eq!(
            interpret(Some((PlayerState::Idle, None, Some(12.5))), 3.0, d),
            status("buffering")
        );
        // No time reported: keep the last one.
        let kept = interpret(Some((PlayerState::Playing, None, None)), 7.0, d);
        assert_eq!(
            kept,
            Event::Status {
                time: 7.0,
                duration: d,
                state: "playing"
            }
        );
    }

    #[test]
    fn idle_with_reason_or_no_item_ends_the_session() {
        let end = |finished| Event::Ended {
            time: 40.0,
            finished,
        };
        assert_eq!(
            interpret(
                Some((PlayerState::Idle, Some(IdleReason::Finished), None)),
                40.0,
                None
            ),
            end(true)
        );
        for reason in [
            IdleReason::Cancelled,
            IdleReason::Interrupted,
            IdleReason::Error,
        ] {
            assert_eq!(
                interpret(
                    Some((PlayerState::Idle, Some(reason), Some(0.0))),
                    40.0,
                    None
                ),
                end(false)
            );
        }
        assert_eq!(interpret(None, 40.0, None), end(false));
    }

    #[test]
    fn prefers_interface_on_target_subnet() {
        let ifaces = [
            ("en0", ip("10.0.0.5"), ip(MASK24)),
            ("en1", ip("192.168.1.20"), ip(MASK24)),
        ];
        assert_eq!(
            pick_lan_ip(&ifaces, Some(ip("192.168.1.99").into())),
            Some(ip("192.168.1.20"))
        );
        assert_eq!(
            pick_lan_ip(&ifaces, Some(ip("10.0.0.7").into())),
            Some(ip("10.0.0.5"))
        );
    }

    #[test]
    fn never_picks_tunnels_or_loopback() {
        let ifaces = [
            ("lo0", ip("127.0.0.1"), ip("255.0.0.0")),
            ("utun4", ip("192.168.1.2"), ip(MASK24)),
            ("wg0", ip("10.8.0.2"), ip(MASK24)),
            ("en0", ip("192.168.1.20"), ip(MASK24)),
        ];
        assert_eq!(
            pick_lan_ip(&ifaces, Some(ip("192.168.1.99").into())),
            Some(ip("192.168.1.20"))
        );
        assert_eq!(pick_lan_ip(&ifaces, None), Some(ip("192.168.1.20")));
        assert_eq!(pick_lan_ip(&ifaces[..3], None), None);
    }

    #[test]
    fn without_subnet_match_falls_back_to_private_address() {
        let ifaces = [
            ("en5", ip("203.0.113.9"), ip(MASK24)),
            ("en0", ip("172.16.4.2"), ip(MASK24)),
        ];
        assert_eq!(
            pick_lan_ip(&ifaces, Some(ip("192.168.1.99").into())),
            Some(ip("172.16.4.2"))
        );
        assert_eq!(pick_lan_ip(&ifaces, None), Some(ip("172.16.4.2")));
        assert_eq!(pick_lan_ip(&ifaces[..1], None), None);
    }

    #[test]
    fn an_ipv6_target_matches_no_subnet() {
        let ifaces = [
            ("en0", ip("172.16.4.2"), ip(MASK24)),
            ("en1", ip("192.168.1.20"), ip(MASK24)),
        ];
        let v6: IpAddr = "fe80::1".parse().unwrap();
        assert_eq!(pick_lan_ip(&ifaces, Some(v6)), Some(ip("172.16.4.2")));
        assert_eq!(
            pick_lan_ip(&ifaces, Some(ip("192.168.1.99").into())),
            Some(ip("192.168.1.20"))
        );
    }
}

/// A fake Cast receiver: TLS with a self-signed certificate, CASTV2 framing
/// (4-byte length + protobuf `CastMessage`) and JSON payloads, enough of the
/// Default Media Receiver for a whole session.
#[cfg(test)]
mod fake {
    use protobuf::Message;
    use rust_cast::cast::cast_channel::{CastMessage, cast_message};
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    pub struct State {
        pub received: Vec<(String, Value)>,
        pub player: &'static str,
        pub time: f64,
        pub level: f64,
        pub muted: bool,
        pub app_running: bool,
        /// Next media status reports this idle reason.
        pub idle_reason: Option<&'static str>,
        /// Next heartbeat is preceded by a receiver status without our app.
        pub taken_over: bool,
        /// Next heartbeat is preceded by a receiver status with our app and
        /// this volume, as when the phone changes it.
        pub volume_elsewhere: Option<f64>,
    }

    fn app() -> Value {
        json!({"appId": "CC1AD845", "displayName": "Default Media Receiver", "sessionId": "session-1",
               "transportId": "transport-1", "statusText": "", "namespaces": [{"name": "urn:x-cast:com.google.cast.media"}]})
    }

    fn receiver_status(s: &State, request_id: Value) -> Value {
        let apps = if s.app_running { vec![app()] } else { vec![] };
        json!({"type": "RECEIVER_STATUS", "requestId": request_id,
               "status": {"applications": apps, "volume": {"level": s.level, "muted": s.muted}}})
    }

    fn media_status(s: &State, request_id: Value) -> Value {
        let mut entry = json!({"mediaSessionId": 7, "playbackRate": 1, "playerState": s.player,
                               "currentTime": s.time, "supportedMediaCommands": 15});
        if let Some(reason) = s.idle_reason {
            entry["playerState"] = json!("IDLE");
            entry["idleReason"] = json!(reason);
        }
        json!({"type": "MEDIA_STATUS", "requestId": request_id, "status": [entry]})
    }

    const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";

    /// Replies as `(namespace, payload)`; `None` answers on the request's namespace.
    fn reply(
        state: &Mutex<State>,
        namespace: &str,
        msg: &Value,
    ) -> Vec<(Option<&'static str>, Value)> {
        let mut s = state.lock().unwrap();
        let kind = msg["type"].as_str().unwrap_or_default().to_string();
        s.received.push((kind.clone(), msg.clone()));
        let id = msg["requestId"].clone();
        match (
            namespace.rsplit('.').next().unwrap_or_default(),
            kind.as_str(),
        ) {
            ("heartbeat", "PING") => {
                let mut out = vec![];
                if let Some(level) = s.volume_elsewhere.take() {
                    s.level = level;
                    out.push((Some(RECEIVER), receiver_status(&s, json!(0))));
                }
                if std::mem::take(&mut s.taken_over) {
                    s.app_running = false;
                    out.push((Some(RECEIVER), receiver_status(&s, json!(0))));
                }
                out.push((None, json!({"type": "PONG"})));
                out
            }
            ("receiver", "LAUNCH") => {
                s.app_running = true;
                vec![(None, receiver_status(&s, id))]
            }
            ("receiver", "GET_STATUS") => vec![(None, receiver_status(&s, id))],
            ("receiver", "SET_VOLUME") => {
                if let Some(l) = msg["volume"]["level"].as_f64() {
                    s.level = l;
                }
                if let Some(m) = msg["volume"]["muted"].as_bool() {
                    s.muted = m;
                }
                vec![(None, receiver_status(&s, id))]
            }
            ("receiver", "STOP") => {
                s.app_running = false;
                vec![(None, receiver_status(&s, id))]
            }
            ("media", "LOAD") => {
                s.player = "PLAYING";
                s.time = msg["currentTime"].as_f64().unwrap_or(0.0);
                vec![(None, media_status(&s, id))]
            }
            ("media", "PLAY") => {
                s.player = "PLAYING";
                vec![(None, media_status(&s, id))]
            }
            ("media", "PAUSE") => {
                s.player = "PAUSED";
                vec![(None, media_status(&s, id))]
            }
            ("media", "SEEK") => {
                s.time = msg["currentTime"].as_f64().unwrap_or(s.time);
                vec![(None, media_status(&s, id))]
            }
            ("media", "GET_STATUS") => vec![(None, media_status(&s, id))],
            _ => vec![],
        }
    }

    /// Starts the receiver on a free local port; it serves one connection.
    pub fn start() -> (u16, Arc<Mutex<State>>) {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
        let config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert.cert.der().clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            player: "IDLE",
            level: 0.5,
            ..Default::default()
        }));
        let shared = state.clone();
        std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(config).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            loop {
                let mut len = [0u8; 4];
                if tls.read_exact(&mut len).is_err() {
                    return;
                }
                let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
                if tls.read_exact(&mut buf).is_err() {
                    return;
                }
                let msg = CastMessage::parse_from_bytes(&buf).unwrap();
                let payload: Value =
                    serde_json::from_str(msg.payload_utf8()).unwrap_or(Value::Null);
                for (namespace, out) in reply(&shared, msg.namespace(), &payload) {
                    let mut answer = CastMessage::new();
                    answer.set_protocol_version(cast_message::ProtocolVersion::CASTV2_1_0);
                    answer.set_source_id(msg.destination_id().into());
                    answer.set_destination_id(msg.source_id().into());
                    answer.set_namespace(namespace.unwrap_or(msg.namespace()).into());
                    answer.set_payload_type(cast_message::PayloadType::STRING);
                    answer.set_payload_utf8(out.to_string());
                    let bytes = answer.write_to_bytes().unwrap();
                    let framed = [&(bytes.len() as u32).to_be_bytes()[..], &bytes].concat();
                    if tls.write_all(&framed).and_then(|_| tls.flush()).is_err() {
                        return;
                    }
                }
            }
        });
        (port, state)
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Events = Arc<Mutex<Vec<Event>>>;

    fn begin(start: f64) -> (mpsc::Sender<Command>, Events, Arc<Mutex<fake::State>>) {
        let (port, state) = fake::start();
        let device = Device {
            name: "Fake".into(),
            ip: "127.0.0.1".parse().unwrap(),
            port,
        };
        let (tx, rx) = mpsc::channel();
        let events: Events = Arc::default();
        let sink = events.clone();
        let load = Load {
            path: "/v.mp4".into(),
            content_type: "video/mp4".into(),
            start,
            duration: Some(100.0),
        };
        play(&device, "http://host:1", load, rx, move |e| {
            sink.lock().unwrap().push(e)
        })
        .unwrap();
        (tx, events, state)
    }

    /// Waits up to 5 s for an event matching `want`.
    fn wait_for(events: &Events, want: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(e) = events.lock().unwrap().iter().find(|e| want(e)) {
                return e.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no matching event in {:?}",
                events.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn session_loads_reports_and_obeys_commands() {
        let (tx, events, state) = begin(12.0);
        {
            let s = state.lock().unwrap();
            let load = &s.received.iter().find(|(k, _)| k == "LOAD").unwrap().1;
            assert_eq!(load["media"]["contentId"], "http://host:1/v.mp4");
            assert_eq!(load["currentTime"], 12.0);
        }
        wait_for(&events, |e| {
            *e == Event::Volume {
                level: 0.5,
                muted: false,
            }
        });
        wait_for(
            &events,
            |e| matches!(e, Event::Status { time, state: "playing", duration: Some(d) } if *time == 12.0 && *d == 100.0),
        );

        tx.send(Command::Pause).unwrap();
        wait_for(&events, |e| {
            matches!(
                e,
                Event::Status {
                    state: "paused",
                    ..
                }
            )
        });
        tx.send(Command::Seek(30.0)).unwrap();
        wait_for(
            &events,
            |e| matches!(e, Event::Status { time, .. } if *time == 30.0),
        );
        tx.send(Command::Play).unwrap();
        wait_for(
            &events,
            |e| matches!(e, Event::Status { time, state: "playing", .. } if *time == 30.0),
        );
        tx.send(Command::Volume(0.25)).unwrap();
        wait_for(&events, |e| {
            *e == Event::Volume {
                level: 0.25,
                muted: false,
            }
        });
        tx.send(Command::Mute(true)).unwrap();
        wait_for(&events, |e| {
            *e == Event::Volume {
                level: 0.25,
                muted: true,
            }
        });

        state.lock().unwrap().idle_reason = Some("FINISHED");
        wait_for(&events, |e| {
            *e == Event::Ended {
                time: 30.0,
                finished: true,
            }
        });
    }

    #[test]
    fn another_file_loads_in_the_same_session() {
        let (tx, events, state) = begin(12.0);
        wait_for(&events, |e| matches!(e, Event::Status { .. }));
        let next = Load {
            path: String::new(),
            content_type: "application/vnd.apple.mpegurl".into(),
            start: 0.0,
            duration: Some(50.0),
        };
        reload(&tx, "http://host:1/hls/2/index.m3u8".into(), next).unwrap();
        {
            let s = state.lock().unwrap();
            let loads: Vec<_> = s.received.iter().filter(|(k, _)| k == "LOAD").collect();
            assert_eq!(loads.len(), 2);
            assert_eq!(
                loads[1].1["media"]["contentId"],
                "http://host:1/hls/2/index.m3u8"
            );
            assert_eq!(
                s.received.iter().filter(|(k, _)| k == "LAUNCH").count(),
                1,
                "app not relaunched"
            );
            assert!(s.app_running);
        }
        wait_for(
            &events,
            |e| matches!(e, Event::Status { duration: Some(d), .. } if *d == 50.0),
        );
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Ended { .. }))
        );
    }

    #[test]
    fn stop_ends_the_session_and_closes_the_app() {
        let (tx, events, state) = begin(0.0);
        wait_for(&events, |e| matches!(e, Event::Status { .. }));
        stop(&tx);
        assert!(
            !state.lock().unwrap().app_running,
            "app closed before stop returns"
        );
        wait_for(&events, |e| {
            matches!(
                e,
                Event::Ended {
                    finished: false,
                    ..
                }
            )
        });
    }

    #[test]
    fn another_app_taking_the_receiver_ends_the_session() {
        let (_tx, events, state) = begin(5.0);
        wait_for(&events, |e| matches!(e, Event::Status { .. }));
        state.lock().unwrap().taken_over = true;
        wait_for(
            &events,
            |e| matches!(e, Event::Ended { time, finished: false } if *time == 5.0),
        );
    }

    #[test]
    fn volume_changed_elsewhere_is_reported_and_the_session_goes_on() {
        let (_tx, events, state) = begin(0.0);
        wait_for(&events, |e| matches!(e, Event::Status { .. }));
        state.lock().unwrap().volume_elsewhere = Some(0.8);
        wait_for(&events, |e| {
            *e == Event::Volume {
                level: 0.8,
                muted: false,
            }
        });
        // The session keeps reporting after the broadcast.
        let seen = events.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(1200));
        let events = events.lock().unwrap();
        assert!(events.len() > seen);
        assert!(!events.iter().any(|e| matches!(e, Event::Ended { .. })));
    }

    #[test]
    fn a_dropped_session_closes_the_app_quietly() {
        let (tx, events, state) = begin(0.0);
        wait_for(&events, |e| matches!(e, Event::Status { .. }));
        drop(tx);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Ended { .. }))
        );
        assert!(!state.lock().unwrap().app_running);
    }

    #[test]
    fn discovers_a_cast_device_announced_on_mdns() {
        let mdns = mdns_sd::ServiceDaemon::new().unwrap();
        let ip = lan_ip(None);
        let service = mdns_sd::ServiceInfo::new(
            "_googlecast._tcp.local.",
            "Tonic-Test-TV",
            "tonic-test-tv.local.",
            ip.to_string().as_str(),
            8009,
            &[("fn", "Tonic Test TV"), ("md", "Chromecast")][..],
        )
        .unwrap();
        mdns.register(service).unwrap();
        let found = discover(Duration::from_secs(3)).unwrap();
        let _ = mdns.shutdown();
        let tv = found
            .iter()
            .find(|d| d.name == "Tonic Test TV")
            .expect("test device found");
        assert_eq!(tv.port, 8009);
        assert_eq!(tv.ip, IpAddr::V4(ip));
        assert_eq!(
            found.iter().filter(|d| d.name == "Tonic Test TV").count(),
            1,
            "listed once"
        );
    }
}
