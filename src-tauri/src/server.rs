use std::{
    io::SeekFrom,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use librqbit::{Session, api::TorrentIdOrHash};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use anyhow::Context;

use crate::{media, transcode};

pub struct Server {
    session: Arc<Session>,
    /// None when ffmpeg is missing.
    transcoder: Option<Arc<transcode::Transcoder>>,
    /// Local files the user opened. Only these are served, by index.
    files: Mutex<Vec<PathBuf>>,
    /// Own port, for ffmpeg to read sources back from this server.
    port: std::sync::atomic::AtomicU16,
    /// Random per run and first in every path: the server answers the whole
    /// LAN (and any page in a local browser), so paths must not be guessable.
    token: String,
}

/// 128 random bits from the OS, as hex.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source is unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Server {
    pub fn stream_path(&self, torrent: usize, file: usize) -> String {
        format!("/{}/stream/{torrent}/{file}", self.token)
    }

    pub fn hls_path(&self, job: u64) -> String {
        format!("/{}/hls/{job}/index.m3u8", self.token)
    }

    /// Preview frames of `source`; `{t}` stands for the time in seconds.
    pub fn thumb_path(&self, source: &str) -> String {
        let rest = source
            .strip_prefix(&format!("/{}/", self.token))
            .unwrap_or_default();
        format!("/{}/thumb/{{t}}/{rest}", self.token)
    }

    /// Whether `path` is a source this server serves (a file or torrent file).
    pub fn is_served(&self, path: &str) -> bool {
        path.strip_prefix(&format!("/{}/", self.token))
            .is_some_and(is_source_path)
    }

    /// Registers a local file and returns its path on the server.
    pub fn add_file(&self, path: PathBuf) -> String {
        let mut files = self.files.lock().unwrap();
        let id = match files.iter().position(|p| *p == path) {
            Some(i) => i,
            None => {
                files.push(path);
                files.len() - 1
            }
        };
        format!("/{}/file/{id}", self.token)
    }
}

/// Serves torrent files and opened local files over HTTP with Range support,
/// so that both the webview and a Chromecast can seek. Binds every interface
/// because the Chromecast fetches the URL itself; returns the chosen port.
pub async fn start(
    session: Arc<Session>,
    transcoder: Option<Arc<transcode::Transcoder>>,
) -> anyhow::Result<(Arc<Server>, u16)> {
    let token = random_token();
    let server = Arc::new(Server {
        session,
        transcoder,
        files: Mutex::default(),
        port: Default::default(),
        token: token.clone(),
    });
    let routes = Router::new()
        .route("/stream/{torrent_id}/{file_id}", get(torrent_file))
        .route("/file/{id}", get(local_file))
        .route("/hls/{job}/{name}", get(hls_file))
        .route("/thumb/{seconds}/{*source}", get(thumbnail))
        .with_state(server.clone());
    let app = Router::new().nest(&format!("/{token}"), routes);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    server
        .port
        .store(port, std::sync::atomic::Ordering::Relaxed);
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok((server, port))
}

async fn torrent_file(
    State(server): State<Arc<Server>>,
    Path((torrent_id, file_id)): Path<(usize, usize)>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let handle = server
            .session
            .get(TorrentIdOrHash::Id(torrent_id))
            .ok_or_else(|| anyhow::anyhow!("unknown torrent"))?;
        let (len, mime) = handle
            .with_metadata(|m| {
                let f = m.file_infos.get(file_id)?;
                Some((f.len, media::content_type(&f.relative_filename)))
            })?
            .ok_or_else(|| anyhow::anyhow!("unknown file"))?;
        let reader = handle.stream(file_id).await?;
        respond(reader, len, mime, &headers).await
    };
    result.await.unwrap_or_else(not_found)
}

async fn local_file(
    State(server): State<Arc<Server>>,
    Path(id): Path<usize>,
    headers: HeaderMap,
) -> Response {
    let Some(path) = server.files.lock().unwrap().get(id).cloned() else {
        return not_found(anyhow::anyhow!("unknown file"));
    };
    let result = async {
        let file = tokio::fs::File::open(&path).await?;
        let len = file.metadata().await?.len();
        respond(file, len, media::content_type(&path), &headers).await
    };
    result.await.unwrap_or_else(not_found)
}

async fn hls_file(
    State(server): State<Arc<Server>>,
    Path((job, name)): Path<(u64, String)>,
    headers: HeaderMap,
) -> Response {
    let (Some(t), Some(file)) = (&server.transcoder, transcode::parse_hls_name(&name)) else {
        return not_found(anyhow::anyhow!("unknown file"));
    };
    let result = async {
        let (path, mime) = match file {
            transcode::HlsFile::Playlist => (
                t.playlist_path(job).await.context("unknown job")?,
                "application/vnd.apple.mpegurl",
            ),
            transcode::HlsFile::Segment(i) => (t.segment(job, i).await?, "video/mp2t"),
        };
        let file = tokio::fs::File::open(&path).await?;
        let len = file.metadata().await?.len();
        respond(file, len, Some(mime), &headers).await
    };
    result.await.unwrap_or_else(not_found)
}

/// A preview frame of an opened source. `source` must be a path this server
/// serves (`file/N` or `stream/T/F`), so no other input reaches ffmpeg.
async fn thumbnail(
    State(server): State<Arc<Server>>,
    Path((seconds, source)): Path<(f64, String)>,
) -> Response {
    let (Some(t), true) = (&server.transcoder, is_source_path(&source)) else {
        return not_found(anyhow::anyhow!("unknown source"));
    };
    let port = server.port.load(std::sync::atomic::Ordering::Relaxed);
    let input = format!("http://127.0.0.1:{port}/{}/{source}", server.token);
    match t.thumbnail(&input, seconds).await {
        Ok(None) => (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response(),
        Ok(Some(jpeg)) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "max-age=3600")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Body::from(jpeg))
            .unwrap_or_else(|e| not_found(e.into())),
        Err(e) => not_found(e),
    }
}

/// `file/<digits>` or `stream/<digits>/<digits>`.
fn is_source_path(path: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["file", id] => digits(id),
        ["stream", t, f] => digits(t) && digits(f),
        _ => false,
    }
}

fn not_found(e: anyhow::Error) -> Response {
    (StatusCode::NOT_FOUND, format!("{e:#}")).into_response()
}

async fn respond<R>(
    mut reader: R,
    len: u64,
    mime: Option<&str>,
    headers: &HeaderMap,
) -> anyhow::Result<Response>
where
    R: AsyncRead + AsyncSeek + Unpin + Send + 'static,
{
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let mut res = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        // The Cast receiver fetches subtitles and HLS parts with CORS.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::CONTENT_TYPE,
            mime.unwrap_or("application/octet-stream"),
        );

    let (start, end) = match media::parse_range(range, len) {
        Err(()) => {
            return Ok(res
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())?);
        }
        Ok(Some((s, e))) => {
            res = res
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {s}-{e}/{len}"));
            (s, e)
        }
        Ok(None) => (0, len.saturating_sub(1)),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };

    reader.seek(SeekFrom::Start(start)).await?;
    let body = Body::from_stream(ReaderStream::new(reader.take(count)));
    Ok(res
        .header(header::CONTENT_LENGTH, HeaderValue::from(count))
        .body(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::io::Cursor;

    #[test]
    fn tokens_are_128_bit_hex_and_never_repeat() {
        let tokens: std::collections::HashSet<String> = (0..1000).map(|_| random_token()).collect();
        assert_eq!(tokens.len(), 1000);
        for t in &tokens {
            assert_eq!(t.len(), 32);
            assert!(
                t.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
        }
    }

    async fn get(range: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let data: Vec<u8> = (0..=99).collect();
        let mut headers = HeaderMap::new();
        if let Some(r) = range {
            headers.insert(header::RANGE, r.parse().unwrap());
        }
        let res = respond(Cursor::new(data), 100, Some("video/mp4"), &headers)
            .await
            .unwrap();
        let (parts, body) = res.into_parts();
        let bytes = to_bytes(body, usize::MAX).await.unwrap().to_vec();
        (parts.status, parts.headers, bytes)
    }

    #[test]
    fn thumbnails_only_for_served_sources() {
        assert!(is_source_path("file/0"));
        assert!(is_source_path("stream/3/12"));
        for bad in [
            "file/",
            "file/x",
            "file/1/2",
            "stream/1",
            "stream/1/x",
            "hls/1/index.m3u8",
            "../etc",
            "",
            "file/1?x",
        ] {
            assert!(!is_source_path(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn whole_body_without_range() {
        let (status, headers, body) = get(None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_LENGTH], "100");
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        assert_eq!(headers[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(body, (0..=99).collect::<Vec<u8>>());
    }

    #[tokio::test]
    async fn partial_body_for_range() {
        let (status, headers, body) = get(Some("bytes=10-19")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 10-19/100");
        assert_eq!(headers[header::CONTENT_LENGTH], "10");
        assert_eq!(body, (10..=19).collect::<Vec<u8>>());
    }

    /// Sends a raw HTTP/1.1 GET and returns the status line and body.
    async fn http(port: u16, path: &str, range: &str) -> (String, Vec<u8>) {
        use tokio::io::AsyncWriteExt;
        let mut conn = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: t\r\nRange: {range}\r\nConnection: close\r\n\r\n"
        );
        conn.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        conn.read_to_end(&mut raw).await.unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let status = head.lines().next().unwrap().to_string();
        (status, raw[split + 4..].to_vec())
    }

    /// The preview route serves a JPEG of a registered local file, as the
    /// window asks for it.
    #[tokio::test]
    async fn thumbnail_of_a_local_file() {
        let dir = std::env::temp_dir().join(format!("tonic-server-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Ok(t) = crate::transcode::Transcoder::new(dir.join("hls")) else {
            return;
        };
        let src = dir.join("in.mp4");
        let made = std::process::Command::new(t.ffmpeg_path())
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=25:duration=4",
            ])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let opts = librqbit::SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            ..Default::default()
        };
        let session = Session::new_with_opts(dir.join("dl"), opts).await.unwrap();
        let (server, port) = start(session, Some(t)).await.unwrap();
        let source = server.add_file(src);
        let (status, body) = http(
            port,
            &server.thumb_path(&source).replace("{t}", "2"),
            "bytes=0-",
        )
        .await;
        assert!(
            status.contains("200"),
            "{status}: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(&body[..2], &[0xFF, 0xD8], "JPEG start marker");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn serves_only_registered_files() {
        let dir = std::env::temp_dir().join(format!("tonic-server-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.mp4"), dir.join("b.mp4"));
        std::fs::write(&a, b"0123456789").unwrap();
        std::fs::write(&b, b"abcdefghij").unwrap();

        let opts = librqbit::SessionOptions {
            dht: None,
            persistence: None,
            listen: None,
            disable_local_service_discovery: true,
            ..Default::default()
        };
        let session = Session::new_with_opts(dir.join("dl"), opts).await.unwrap();
        let (server, port) = start(session, None).await.unwrap();

        let path_a = server.add_file(a.clone());
        let path_b = server.add_file(b);
        assert_ne!(path_a, path_b);
        assert_eq!(server.add_file(a), path_a, "same file keeps its path");

        let (status, body) = http(port, &path_a, "bytes=2-4").await;
        assert!(status.contains("206"), "{status}");
        assert_eq!(body, b"234");
        let (status, body) = http(port, &path_b, "bytes=0-2").await;
        assert!(status.contains("206"), "{status}");
        assert_eq!(body, b"abc");

        let (status, _) = http(port, &path_a.replace("/file/0", "/file/99"), "bytes=0-1").await;
        assert!(status.contains("404"), "{status}");
        let (status, _) = http(port, &server.stream_path(0, 0), "bytes=0-1").await;
        assert!(status.contains("404"), "{status}");

        let (status, _) = http(port, &server.hls_path(1), "bytes=0-").await;
        assert!(status.contains("404"), "{status}");
        // Without the token nothing is served.
        let (status, _) = http(port, "/file/0", "bytes=0-1").await;
        assert!(status.contains("404"), "{status}");
        assert!(server.is_served(&path_a) && !server.is_served("/file/0"));
        assert!(server.thumb_path(&path_a).ends_with("/thumb/{t}/file/0"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn unsatisfiable_range() {
        let (status, headers, body) = get(Some("bytes=500-")).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes */100");
        assert!(body.is_empty());
    }
}

/// Needs the network: `cargo test -- --ignored real_torrent --nocapture`.
#[cfg(test)]
mod network {
    use super::*;
    use std::time::Duration;

    const SINTEL: &str = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.openbittorrent.com%3A6969";

    #[tokio::test]
    #[ignore]
    async fn real_torrent_streams_and_probes() {
        let dir = std::env::temp_dir().join(format!("tonic-net-{}", std::process::id()));
        let opts = librqbit::SessionOptions {
            dht: Some(librqbit::DhtSessionConfig {
                persistence: None,
                ..Default::default()
            }),
            persistence: None,
            ..Default::default()
        };
        let session = Session::new_with_opts(dir.clone(), opts).await.unwrap();
        let add = librqbit::AddTorrentOptions {
            overwrite: true,
            ..Default::default()
        };
        let added = tokio::time::timeout(
            Duration::from_secs(120),
            session.add_torrent(librqbit::AddTorrent::from_url(SINTEL), Some(add)),
        )
        .await
        .expect("metadata within 2 min")
        .unwrap();
        let (id, handle) = match added {
            librqbit::AddTorrentResponse::Added(id, h)
            | librqbit::AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            _ => panic!("list only"),
        };
        let files: Vec<_> = handle
            .with_metadata(|m| {
                m.file_infos
                    .iter()
                    .map(|f| (f.relative_filename.clone(), f.len))
                    .collect()
            })
            .unwrap();
        eprintln!("files: {files:#?}");
        let videos = media::video_files(files.iter().map(|(p, _)| p.as_path()));
        eprintln!("videos: {videos:?}");
        handle.wait_until_initialized().await.unwrap();
        let only = std::collections::HashSet::from([videos[0]]);
        session.update_only_files(&handle, &only).await.unwrap();

        let (server, port) = start(session.clone(), None).await.unwrap();
        let url = format!(
            "http://127.0.0.1:{port}{}",
            server.stream_path(id, videos[0])
        );
        let out = tokio::time::timeout(
            Duration::from_secs(180),
            tokio::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=format_name,duration:stream=codec_name",
                    "-of",
                    "compact",
                    &url,
                ])
                .output(),
        )
        .await
        .expect("ffprobe within 3 min")
        .unwrap();
        eprintln!(
            "ffprobe status {}\nstdout {}\nstderr {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stats = handle.stats();
        eprintln!(
            "progress {}/{} peers {:?}",
            stats.progress_bytes,
            stats.total_bytes,
            stats.live.map(|l| l.snapshot.peer_stats.live)
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.status.success());
    }
}
