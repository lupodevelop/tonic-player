use std::path::Path;

use librqbit::{AddTorrent, ConnectionOptions, DhtSessionConfig, SessionOptions};
use serde::{Deserialize, Serialize};

/// The shield sends every torrent connection through a SOCKS5 proxy. It is
/// the only setting kept between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Shield {
    pub on: bool,
    pub proxy: String,
}

impl Shield {
    /// Off when there is no settings file. A file that cannot be read gives a
    /// shield that is on with no proxy, so torrents are refused rather than
    /// run in the clear.
    pub fn load(path: &Path) -> Shield {
        match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Shield::default(),
            read => read
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Shield {
                    on: true,
                    proxy: String::new(),
                }),
        }
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// The proxy to use, `None` when the shield is off. An unusable proxy with
    /// the shield on is an error, so torrents never run unshielded.
    pub fn proxy(&self) -> Result<Option<&str>, String> {
        if !self.on {
            return Ok(None);
        }
        check_proxy(&self.proxy)?;
        Ok(Some(&self.proxy))
    }
}

pub fn check_proxy(proxy: &str) -> Result<(), String> {
    let url = url::Url::parse(proxy)
        .map_err(|_| "The proxy must look like socks5h://host:port".to_string())?;
    if !matches!(url.scheme(), "socks5" | "socks5h") {
        return Err("The proxy must be a socks5:// or socks5h:// address".into());
    }
    if url.host_str().is_none_or(str::is_empty) || url.port().is_none() {
        return Err("The proxy needs a host and a port".into());
    }
    Ok(())
}

/// No session or DHT state on disk, and no local peer discovery: its
/// multicast would raise macOS's local network prompt at launch. With the
/// shield on only TCP through the proxy is left (no DHT, no uTP, no
/// listener), because the proxy carries nothing else; UDP trackers are
/// dropped per torrent. An unusable proxy leaves no peer traffic at all.
pub fn session_opts(shield: &Shield) -> SessionOptions {
    let mut opts = SessionOptions {
        dht: Some(DhtSessionConfig {
            persistence: None,
            ..Default::default()
        }),
        disable_local_service_discovery: true,
        ..Default::default()
    };
    if shield.on {
        opts.dht = None;
    }
    if let Ok(Some(proxy)) = shield.proxy() {
        opts.connect = Some(ConnectionOptions {
            proxy_url: Some(proxy.to_owned()),
            ..Default::default()
        });
    }
    opts
}

fn over_tcp(tracker: &str) -> bool {
    url::Url::parse(tracker).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

const NO_TRACKER: &str =
    "With the shield on this torrent cannot find peers: it has no HTTP tracker";

/// The magnet with its HTTP trackers only.
pub fn shield_magnet(magnet: &str) -> Result<String, String> {
    let url = url::Url::parse(magnet).map_err(|_| "Invalid magnet link".to_string())?;
    let mut kept = url::form_urlencoded::Serializer::new(String::new());
    let mut tracker = false;
    for (key, value) in url.query_pairs() {
        if key == "tr" {
            if !over_tcp(&value) {
                continue;
            }
            tracker = true;
        }
        kept.append_pair(&key, &value);
    }
    if !tracker {
        return Err(NO_TRACKER.into());
    }
    Ok(format!("magnet:?{}", kept.finish()))
}

/// What to add to a session with this shield: magnets and torrent files lose
/// their non-HTTP trackers, whose HTTP trackers come back as the second value
/// for a torrent file. Anything else is refused, since it would be fetched
/// without filtering.
pub fn prepare<'a>(
    shield: &Shield,
    add: AddTorrent<'a>,
) -> Result<(AddTorrent<'a>, Option<Vec<String>>), String> {
    if shield.proxy()?.is_none() {
        return Ok((add, None));
    }
    match add {
        AddTorrent::Url(url) if crate::media::is_magnet(&url) => {
            Ok((AddTorrent::from_url(shield_magnet(url.trim())?), None))
        }
        AddTorrent::TorrentFileBytes(bytes) => {
            let (bytes, trackers) = shield_torrent(&bytes)?;
            Ok((AddTorrent::from_bytes(bytes), Some(trackers)))
        }
        AddTorrent::Url(_) => Err("The shield takes magnet links and torrent files only".into()),
    }
}

/// The torrent without its trackers, plus its HTTP trackers to add back. The
/// info dict is copied byte for byte, so the info hash does not change.
pub fn shield_torrent(bytes: &[u8]) -> Result<(Vec<u8>, Vec<String>), String> {
    let torrent =
        librqbit::torrent_from_bytes(bytes).map_err(|e| format!("Invalid torrent file: {e}"))?;
    let mut trackers: Vec<String> = torrent
        .iter_announce()
        .filter_map(|t| std::str::from_utf8(t.as_ref()).ok())
        .filter(|t| over_tcp(t))
        .map(str::to_owned)
        .collect();
    trackers.sort_unstable();
    trackers.dedup();
    if trackers.is_empty() {
        return Err(NO_TRACKER.into());
    }
    let info = torrent.info.raw_bytes.as_ref();
    let mut stripped = Vec::with_capacity(info.len() + 8);
    stripped.extend_from_slice(b"d4:info");
    stripped.extend_from_slice(info);
    stripped.push(b'e');
    Ok((stripped, trackers))
}

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::*;

    const HASH: &str = "08ada5a7a6183aae1e09d831df6748d566095a10";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tonic-shield-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn on(proxy: &str) -> Shield {
        Shield {
            on: true,
            proxy: proxy.into(),
        }
    }

    async fn torrent(dir: &Path, trackers: &[&str]) -> librqbit::CreateTorrentResult {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.mp4"), vec![7u8; 40_000]).unwrap();
        let trackers = trackers.iter().map(|t| t.to_string()).collect();
        let opts = librqbit::CreateTorrentOptions {
            trackers,
            ..Default::default()
        };
        librqbit::create_torrent(&src, opts, &librqbit::spawn_utils::BlockingSpawner::new(1))
            .await
            .unwrap()
    }

    #[test]
    fn proxy_only_when_on_and_valid() {
        assert_eq!(
            Shield {
                on: false,
                proxy: "nonsense".into()
            }
            .proxy(),
            Ok(None)
        );
        for good in [
            "socks5h://u:p@10.0.0.1:1080",
            "socks5://[::1]:1080",
            "socks5h://u:p%40ss@vpn.example:1080",
        ] {
            assert_eq!(on(good).proxy(), Ok(Some(good)));
        }
        for bad in [
            "",
            "   ",
            "http://10.0.0.1:1080",
            "socks4://h:1080",
            "socks5://10.0.0.1",
            "socks5://:1080",
        ] {
            assert!(on(bad).proxy().is_err(), "{bad}");
        }
    }

    #[test]
    fn missing_settings_mean_off_and_unreadable_ones_fail_closed() {
        let dir = scratch("settings");
        let path = dir.join("cfg/shield.json");
        assert_eq!(Shield::load(&path), Shield::default());
        let shield = on("socks5://h:1");
        shield.save(&path).unwrap();
        assert_eq!(Shield::load(&path), shield);
        assert!(!path.with_extension("tmp").exists());
        std::fs::write(&path, b"{\"on\": fal").unwrap();
        let broken = Shield::load(&path);
        assert!(broken.on && broken.proxy().is_err());
        let unreadable = Shield::load(&dir);
        assert!(unreadable.on && unreadable.proxy().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shield_leaves_only_tcp_through_the_proxy() {
        let plain = session_opts(&Shield {
            on: false,
            proxy: "socks5h://h:1080".into(),
        });
        assert!(plain.dht.unwrap().persistence.is_none() && plain.connect.is_none());
        assert!(plain.persistence.is_none() && !plain.fastresume && plain.listen.is_none());
        assert!(plain.disable_local_service_discovery);
        let broken = session_opts(&on(""));
        assert!(broken.dht.is_none() && broken.connect.is_none());
        let opts = session_opts(&on("socks5h://h:1080"));
        assert!(
            opts.dht.is_none() && opts.listen.is_none() && opts.disable_local_service_discovery
        );
        assert_eq!(
            opts.connect.unwrap().proxy_url.as_deref(),
            Some("socks5h://h:1080")
        );
    }

    #[test]
    fn magnet_keeps_http_trackers_only() {
        let magnet = format!(
            "magnet:?xt=urn:btih:{HASH}&dn=Big%20Buck&tr=udp%3A%2F%2Ft.example%3A80&tr=HTTP%3A%2F%2Fa.example%2Fannounce\
             &tr=https%3A%2F%2Ft.example%2Fannounce%3Fk%3D1%26x%3D2&tr=wss%3A%2F%2Fw.example&tr=not%20a%20url"
        );
        let shielded = librqbit::Magnet::parse(&shield_magnet(&magnet).unwrap()).unwrap();
        assert_eq!(
            shielded.trackers,
            [
                "HTTP://a.example/announce",
                "https://t.example/announce?k=1&x=2"
            ]
        );
        assert_eq!(shielded.name.as_deref(), Some("Big Buck"));
        assert_eq!(shielded.as_id20().unwrap().as_string(), HASH);
    }

    #[test]
    fn magnet_without_http_tracker_is_refused() {
        assert!(
            shield_magnet(&format!("magnet:?xt=urn:btih:{HASH}&tr=udp://t.example:80")).is_err()
        );
        assert!(shield_magnet(&format!("magnet:?xt=urn:btih:{HASH}")).is_err());
        assert!(shield_magnet("not a magnet").is_err());
    }

    #[tokio::test]
    async fn torrent_keeps_info_hash_and_http_trackers() {
        let dir = scratch("torrent");
        let made = torrent(
            &dir,
            &[
                "http://t.example/announce",
                "udp://t.example:80",
                "http://t.example/announce",
            ],
        )
        .await;
        let (stripped, http) = shield_torrent(&made.as_bytes().unwrap()).unwrap();
        let parsed = librqbit::torrent_from_bytes(&stripped).unwrap();
        assert_eq!(parsed.info_hash, made.info_hash());
        assert_eq!(parsed.iter_announce().count(), 0);
        assert_eq!(http, ["http://t.example/announce"]);

        let udp_only = torrent(&dir, &["udp://t.example:80"]).await;
        assert!(shield_torrent(&udp_only.as_bytes().unwrap()).is_err());
        assert!(shield_torrent(b"d4:infoi1ee").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn prepare_filters_only_with_the_shield_on() {
        let dir = scratch("prepare");
        let bytes = torrent(&dir, &["udp://t.example:80", "http://t.example/announce"])
            .await
            .as_bytes()
            .unwrap();
        let magnet = format!(
            "magnet:?xt=urn:btih:{HASH}&tr=udp%3A%2F%2Fu.example%3A1&tr=http%3A%2F%2Ft.example%2Fa"
        );
        let url = |a: AddTorrent<'_>| match a {
            AddTorrent::Url(u) => u.into_owned(),
            AddTorrent::TorrentFileBytes(_) => panic!("expected a URL"),
        };

        let off = Shield::default();
        assert_eq!(
            url(prepare(&off, AddTorrent::from_url(magnet.clone()))
                .unwrap()
                .0),
            magnet
        );
        let (same, trackers) = prepare(&off, AddTorrent::from_bytes(bytes.clone())).unwrap();
        assert!(
            matches!(same, AddTorrent::TorrentFileBytes(b) if b == bytes) && trackers.is_none()
        );

        let shield = on("socks5h://127.0.0.1:1080");
        let (m, trackers) =
            prepare(&shield, AddTorrent::from_url(format!("  {magnet}\n"))).unwrap();
        assert_eq!(
            librqbit::Magnet::parse(&url(m)).unwrap().trackers,
            ["http://t.example/a"]
        );
        assert!(trackers.is_none());
        let (_, trackers) = prepare(&shield, AddTorrent::from_bytes(bytes.clone())).unwrap();
        assert_eq!(trackers.unwrap(), ["http://t.example/announce"]);
        assert!(
            prepare(
                &shield,
                AddTorrent::from_url("https://t.example/x.torrent?tr=http%3A%2F%2Ft.example%2Fa")
            )
            .is_err()
        );

        assert!(prepare(&on(""), AddTorrent::from_url(magnet)).is_err());
        assert!(prepare(&on(""), AddTorrent::from_bytes(bytes)).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A SOCKS5 relay without authentication that records where it connects.
    async fn socks5_relay() -> (SocketAddr, Arc<Mutex<Vec<SocketAddr>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let record = record.clone();
                tokio::spawn(async move {
                    let mut head = [0u8; 2];
                    client.read_exact(&mut head).await?;
                    let mut methods = vec![0u8; head[1] as usize];
                    client.read_exact(&mut methods).await?;
                    client.write_all(&[5, 0]).await?;
                    let mut req = [0u8; 4];
                    client.read_exact(&mut req).await?;
                    let host = match req[3] {
                        1 => {
                            let mut ip = [0u8; 4];
                            client.read_exact(&mut ip).await?;
                            std::net::Ipv4Addr::from(ip).to_string()
                        }
                        3 => {
                            let mut name = vec![0u8; client.read_u8().await? as usize];
                            client.read_exact(&mut name).await?;
                            String::from_utf8_lossy(&name).into_owned()
                        }
                        _ => return Ok::<_, std::io::Error>(()),
                    };
                    let port = client.read_u16().await?;
                    let target: SocketAddr = tokio::net::lookup_host((host.as_str(), port))
                        .await?
                        .next()
                        .unwrap();
                    record.lock().unwrap().push(target);
                    let mut upstream = TcpStream::connect(target).await?;
                    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
                    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
                    Ok(())
                });
            }
        });
        (addr, seen)
    }

    /// Accepts connections and counts them; `reply` answers each one.
    async fn counting_server(reply: Option<Vec<u8>>) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let reply = reply.clone();
                tokio::spawn(async move {
                    let Some(reply) = reply else { return };
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = conn.write_all(&reply).await;
                });
            }
        });
        (addr, count)
    }

    /// The tracker and the peer it hands out are reached through the proxy,
    /// and never directly.
    #[tokio::test]
    async fn shielded_session_reaches_tracker_and_peers_only_through_the_proxy() {
        let (proxy, relayed) = socks5_relay().await;
        let (peer, peer_hits) = counting_server(None).await;
        let std::net::IpAddr::V4(ip) = peer.ip() else {
            unreachable!()
        };
        let mut body = b"d8:intervali60e5:peers6:".to_vec();
        body.extend_from_slice(&ip.octets());
        body.extend_from_slice(&peer.port().to_be_bytes());
        body.push(b'e');
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(&body);
        let (tracker, tracker_hits) = counting_server(Some(reply)).await;

        let dir = scratch("session");
        let made = torrent(
            &dir,
            &[
                &format!("http://{tracker}/announce"),
                "udp://127.0.0.1:9/announce",
            ],
        )
        .await;
        let shield = on(&format!("socks5h://{proxy}"));
        let (add, trackers) =
            prepare(&shield, AddTorrent::from_bytes(made.as_bytes().unwrap())).unwrap();
        let session = librqbit::Session::new_with_opts(dir.join("dl"), session_opts(&shield))
            .await
            .unwrap();
        let opts = librqbit::AddTorrentOptions {
            trackers,
            overwrite: true,
            ..Default::default()
        };
        session.add_torrent(add, Some(opts)).await.unwrap();

        let through = |addr: SocketAddr| {
            relayed
                .lock()
                .unwrap()
                .iter()
                .filter(|&&a| a == addr)
                .count()
        };
        tokio::time::timeout(Duration::from_secs(30), async {
            while through(peer) == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the peer from the tracker was never reached through the proxy");
        tokio::time::sleep(Duration::from_secs(1)).await;
        session.stop().await;

        assert!(through(tracker) > 0);
        assert!(
            tracker_hits.load(Ordering::SeqCst) <= through(tracker),
            "tracker reached directly"
        );
        assert!(
            peer_hits.load(Ordering::SeqCst) <= through(peer),
            "peer reached directly"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
