mod cast;
mod media;
mod server;
mod shield;
mod transcode;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, api::TorrentIdOrHash};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

struct AppState {
    session: Arc<Session>,
    /// Gives access to the piece bitmap, which `Session` keeps private.
    api: librqbit::Api,
    server: Arc<server::Server>,
    /// Missing when ffmpeg is not installed; direct playback still works.
    transcoder: Result<Arc<transcode::Transcoder>, String>,
    /// The running Cast session: its id, device and remote control.
    cast: Mutex<Option<CastSession>>,
    /// File selected for download in the torrent being opened or played.
    selected: Mutex<Option<usize>>,
    /// One duration probe at a time: each widens the download selection.
    probing: tokio::sync::Mutex<()>,
    /// Files handed over by the system (Open With, a double click, a second
    /// launch) that the window has not taken yet.
    incoming: Mutex<Vec<String>>,
    port: u16,
    /// The shield as it was at launch: the session is built from it.
    shield: shield::Shield,
    shield_file: PathBuf,
}

/// Everything the app writes (torrent data, HLS segments) lives here and is
/// removed at start and at exit: nothing is kept between runs.
fn scratch_dir() -> PathBuf {
    std::env::temp_dir().join("tonic-player")
}

/// Removes every torrent but `keep`, with its downloaded data.
async fn forget_torrents(state: &AppState, keep: Option<usize>) {
    let old: Vec<usize> = state.session.with_torrents(|all| {
        all.map(|(id, _)| id)
            .filter(|&id| Some(id) != keep)
            .collect()
    });
    for id in old {
        let _ = state.session.delete(TorrentIdOrHash::Id(id), true).await;
    }
}

/// Bounds the steps that wait on the network or a slow torrent.
async fn within<T>(
    secs: u64,
    what: &str,
    work: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    tokio::time::timeout(Duration::from_secs(secs), work)
        .await
        .map_err(|_| format!("{what} timed out"))
}

impl AppState {
    fn transcoder(&self) -> Result<&Arc<transcode::Transcoder>, String> {
        self.transcoder.as_ref().map_err(Clone::clone)
    }

    fn local_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

const HLS: &str = "application/vnd.apple.mpegurl";

#[derive(Serialize, Deserialize, Clone)]
struct Episode {
    file_id: usize,
    /// Path inside the torrent; its extension picks the MIME type.
    name: String,
    /// What the window shows.
    title: String,
    size: u64,
}

/// An opened source. When a torrent holds several videos, `path` is empty and
/// the window asks which one to play from `episodes`.
#[derive(Serialize, Default)]
struct Opened {
    name: String,
    /// What the window plays: the source itself or an HLS transcode of it.
    path: String,
    /// The original file on the stream server.
    source: String,
    /// Preview frame URL path; `{t}` stands for the time in seconds.
    thumbs: String,
    /// LAN address of this machine, so AirPlay receivers can fetch the stream too.
    host: String,
    port: u16,
    torrent: Option<usize>,
    file_id: Option<usize>,
    /// VR layout, when the video looks like one.
    vr: Option<media::Vr>,
    chapters: Vec<transcode::Chapter>,
    /// Videos of the torrent or folder, in natural order; empty for a single video.
    episodes: Vec<Episode>,
    /// The folder the episodes come from; their `name` is then a file path.
    folder: Option<String>,
}

impl Opened {
    fn choose(torrent: usize, episodes: Vec<Episode>, port: u16) -> Self {
        Opened {
            torrent: Some(torrent),
            episodes,
            port,
            host: lan_host(),
            ..Default::default()
        }
    }
}

fn lan_host() -> String {
    cast::lan_ip(None).to_string()
}

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Opens whatever the user dropped or pasted: a magnet link, a `.torrent`
/// file or a local video file.
#[tauri::command]
async fn open(input: String, state: tauri::State<'_, AppState>) -> Result<Opened, String> {
    let input = input.trim();
    if media::is_magnet(input) {
        return open_torrent(&state, AddTorrent::from_url(input.to_string())).await;
    }
    let path = PathBuf::from(input);
    let unknown = || "Not a video, folder, torrent or magnet link".to_string();
    if path.is_dir() {
        return open_folder(&state, &path).await;
    }
    if !path.is_file() {
        return Err(unknown());
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
    {
        let bytes = std::fs::read(&path).map_err(err)?;
        return open_torrent(&state, AddTorrent::from_bytes(bytes)).await;
    }
    if media::content_type(&path).is_none() {
        return Err(unknown());
    }
    open_file(&state, path).await
}

/// A local video; earlier torrents go once it plays.
async fn open_file(state: &AppState, path: PathBuf) -> Result<Opened, String> {
    let mime = media::content_type(&path).ok_or("not a video")?;
    let name: String = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into();
    let source = state.server.add_file(path);
    let opened = opened(state, name.clone(), &name, source, mime).await?;
    forget_torrents(state, None).await;
    Ok(opened)
}

/// The videos directly inside `dir`, in natural order; subfolders are not
/// searched. One video plays at once; several come back as a list to pick
/// from, each opened later by its path.
async fn open_folder(state: &AppState, dir: &std::path::Path) -> Result<Opened, String> {
    let files = media::folder_videos(dir).map_err(err)?;
    let episodes: Vec<Episode> = files
        .iter()
        .enumerate()
        .map(|(i, (path, size))| {
            let title = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            Episode {
                file_id: i,
                name: path.to_string_lossy().into_owned(),
                title,
                size: *size,
            }
        })
        .collect();
    match episodes.as_slice() {
        [] => Err("No videos in this folder".into()),
        [only] => open_file(state, PathBuf::from(&only.name)).await,
        _ => Ok(Opened {
            folder: Some(dir.to_string_lossy().into_owned()),
            episodes,
            port: state.port,
            host: lan_host(),
            ..Default::default()
        }),
    }
}

/// Duration and title tag of a local video, for the folder list.
#[tauri::command]
async fn file_info(path: String, state: tauri::State<'_, AppState>) -> Result<EpisodeInfo, String> {
    let t = state.transcoder()?.clone();
    let probe = tokio::time::timeout(Duration::from_secs(10), t.probe(&path)).await;
    let Some(format) = probe.ok().and_then(Result::ok).map(|p| p.format) else {
        return Ok(EpisodeInfo::default());
    };
    Ok(EpisodeInfo {
        title: format.title().map(str::to_string),
        duration: format.duration.and_then(|d| d.parse().ok()),
    })
}

/// The webview plays MP4, WebM and QuickTime with codecs it decodes; the
/// rest goes through HLS. `file_name` (name or path of the file) helps
/// recognise VR.
async fn opened(
    state: &AppState,
    name: String,
    file_name: &str,
    source: String,
    mime: &str,
) -> Result<Opened, String> {
    let input = state.local_url(&source);
    // A slow source counts as unreadable: it still plays if the webview can
    // take it as it is.
    let probe = match state.transcoder() {
        Ok(t) => Some(
            within(30, "reading the video", t.probe(&input))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!(e))),
        ),
        Err(_) => None,
    };
    let vr = match &probe {
        Some(Ok(p)) => transcode::vr(p, file_name),
        _ => media::detect_vr(file_name, 0, 0, false, None),
    };
    let chapters = match &probe {
        Some(Ok(p)) => transcode::chapters(p),
        _ => vec![],
    };
    let container_plays = matches!(mime, "video/mp4" | "video/webm" | "video/quicktime");
    let codecs_play = match &probe {
        Some(Ok(p)) => transcode::webview_plays(p),
        _ => true,
    };
    let path = if container_plays && codecs_play {
        source.clone()
    } else {
        let t = state.transcoder()?;
        let probe = probe.ok_or("ffmpeg is missing")?.map_err(err)?;
        let plan = transcode::plan(&probe).map_err(err)?;
        let job = t.open(&input, &plan).await.map_err(err)?;
        state.server.hls_path(job)
    };
    let thumbs = state.server.thumb_path(&source);
    Ok(Opened {
        name,
        path,
        source,
        thumbs,
        vr,
        chapters,
        host: lan_host(),
        port: state.port,
        ..Default::default()
    })
}

async fn open_torrent(state: &AppState, add: AddTorrent<'_>) -> Result<Opened, String> {
    let (add, trackers) = shield::prepare(&state.shield, add)?;
    // Overwrite: data left by a crashed run must not block the download.
    let opts = AddTorrentOptions {
        overwrite: true,
        trackers,
        ..Default::default()
    };
    let added = within(
        120,
        "fetching the torrent",
        state.session.add_torrent(add, Some(opts)),
    )
    .await?
    .map_err(err)?;
    let (id, handle) = match added {
        AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
        AddTorrentResponse::ListOnly(_) => return Err("torrent was only listed".into()),
    };
    let episodes = handle
        .with_metadata(|m| {
            let videos =
                media::video_files(m.file_infos.iter().map(|f| f.relative_filename.as_path()));
            let paths: Vec<_> = videos
                .iter()
                .map(|&i| m.file_infos[i].relative_filename.as_path())
                .collect();
            let titles = media::titles(&paths);
            videos
                .into_iter()
                .zip(titles)
                .map(|(i, title)| Episode {
                    file_id: i,
                    name: m.file_infos[i].relative_filename.to_string_lossy().into(),
                    title,
                    size: m.file_infos[i].len,
                })
                .collect::<Vec<_>>()
        })
        .map_err(err)?;
    let first = episodes
        .first()
        .ok_or("No playable video in this torrent")?
        .file_id;
    if episodes.len() > 1 {
        // Download only the first until the user picks; what plays now goes on.
        select_file(state, id, first).await?;
        return Ok(Opened::choose(id, episodes, state.port));
    }
    play_episode(state, id, first, episodes).await
}

/// Downloads only `file_id` of the torrent, so a season is not fetched whole.
/// Waits for a running duration probe, which widens the selection meanwhile.
async fn select_file(state: &AppState, torrent: usize, file_id: usize) -> Result<(), String> {
    let _probe_done = state.probing.lock().await;
    *state.selected.lock().unwrap() = Some(file_id);
    download_only(state, torrent, &[file_id]).await
}

async fn download_only(state: &AppState, torrent: usize, files: &[usize]) -> Result<(), String> {
    let handle = state
        .session
        .get(TorrentIdOrHash::Id(torrent))
        .ok_or("unknown torrent")?;
    // librqbit refuses to change files while it checks existing data.
    handle.wait_until_initialized().await.map_err(err)?;
    let only = files.iter().copied().collect();
    state
        .session
        .update_only_files(&handle, &only)
        .await
        .map_err(err)
}

#[derive(Serialize, Default)]
struct EpisodeInfo {
    /// Seconds.
    duration: Option<f64>,
    /// The title tag inside the file, if any.
    title: Option<String>,
}

/// Duration and title of one video of a torrent, read from its header. The
/// file is added to the download selection only while ffprobe reads it.
#[tauri::command]
async fn episode_info(
    torrent: usize,
    file_id: usize,
    state: tauri::State<'_, AppState>,
) -> Result<EpisodeInfo, String> {
    let t = state.transcoder()?.clone();
    let _one_at_a_time = state.probing.lock().await;
    let selected = *state.selected.lock().unwrap();
    let mut files: Vec<usize> = selected.into_iter().collect();
    files.push(file_id);
    download_only(&state, torrent, &files).await?;
    let input = state.local_url(&state.server.stream_path(torrent, file_id));
    let probe = tokio::time::timeout(Duration::from_secs(10), t.probe(&input)).await;
    // Back to the selection of the file being played.
    let current = *state.selected.lock().unwrap();
    download_only(&state, torrent, &current.into_iter().collect::<Vec<_>>()).await?;
    let Some(format) = probe.ok().and_then(Result::ok).map(|p| p.format) else {
        return Ok(EpisodeInfo::default());
    };
    Ok(EpisodeInfo {
        title: format.title().map(str::to_string),
        duration: format.duration.and_then(|d| d.parse().ok()),
    })
}

async fn play_episode(
    state: &AppState,
    torrent: usize,
    file_id: usize,
    episodes: Vec<Episode>,
) -> Result<Opened, String> {
    let episode = episodes
        .iter()
        .find(|e| e.file_id == file_id)
        .ok_or("unknown file")?;
    let mime = media::content_type(std::path::Path::new(&episode.name)).ok_or("not a video")?;
    select_file(state, torrent, file_id).await?;
    let source = state.server.stream_path(torrent, file_id);
    let mut opened = opened(state, episode.title.clone(), &episode.name, source, mime).await?;
    forget_torrents(state, Some(torrent)).await;
    opened.torrent = Some(torrent);
    opened.file_id = Some(file_id);
    opened.episodes = episodes;
    Ok(opened)
}

/// Plays another video of an open torrent; `episodes` is the list `open` gave.
#[tauri::command]
async fn open_episode(
    torrent: usize,
    file_id: usize,
    episodes: Vec<Episode>,
    state: tauri::State<'_, AppState>,
) -> Result<Opened, String> {
    play_episode(&state, torrent, file_id, episodes).await
}

#[derive(Serialize)]
struct TorrentStats {
    /// Connected peers, and peers known from trackers and DHT.
    peers: u32,
    seen: u32,
    /// Bytes per second.
    down: f64,
    up: f64,
    /// Downloaded and total bytes of the file being played.
    file_done: u64,
    file_len: u64,
    /// Downloaded parts of the file, as `(start, end)` fractions.
    have: Vec<(f64, f64)>,
}

#[tauri::command]
fn torrent_stats(
    torrent: usize,
    file_id: usize,
    state: tauri::State<'_, AppState>,
) -> Result<TorrentStats, String> {
    let handle = state
        .session
        .get(TorrentIdOrHash::Id(torrent))
        .ok_or("unknown torrent")?;
    let stats = handle.stats();
    let (file_len, pieces) = handle
        .with_metadata(|m| {
            m.file_infos
                .get(file_id)
                .map(|f| (f.len, f.piece_range.clone()))
        })
        .map_err(err)?
        .ok_or("unknown file")?;
    // Not available while the torrent initializes.
    let have = match state.api.api_dump_haves(TorrentIdOrHash::Id(torrent)) {
        Ok((bits, _)) => media::have_ranges(pieces, |p| bits.get(p as usize).is_some_and(|b| *b)),
        Err(_) => vec![],
    };
    const MIB: f64 = 1024.0 * 1024.0;
    let live = stats.live.as_ref();
    Ok(TorrentStats {
        peers: live.map_or(0, |l| l.snapshot.peer_stats.live),
        seen: live.map_or(0, |l| l.snapshot.peer_stats.seen),
        down: live.map_or(0.0, |l| l.download_speed.mbps * MIB),
        up: live.map_or(0.0, |l| l.upload_speed.mbps * MIB),
        file_done: stats.file_progress.get(file_id).copied().unwrap_or(0),
        file_len,
        have,
    })
}

#[tauri::command]
async fn discover_devices() -> Result<Vec<cast::Device>, String> {
    tauri::async_runtime::spawn_blocking(|| cast::discover(Duration::from_secs(3)))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
async fn cast_to(
    device: cast::Device,
    source: String,
    start: f64,
    vr: Option<media::Vr>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    // MP4s the device can decode are sent as they are; the rest goes through
    // the same HLS job the window uses. VR is reprojected to a flat view.
    if !state.server.is_served(&source) {
        return Err("unknown source".into());
    }
    let t = state.transcoder()?;
    let input = state.local_url(&source);
    let probe = within(30, "reading the video", t.probe(&input))
        .await?
        .map_err(err)?;
    let mut plan = transcode::plan(&probe).map_err(err)?;
    if vr.is_some() {
        plan.flat = vr;
        plan.direct = false;
    }
    let media = if plan.direct {
        cast::Load {
            path: source,
            content_type: "video/mp4".into(),
            start,
            duration: plan.duration,
        }
    } else {
        let job = t.open(&input, &plan).await.map_err(err)?;
        cast::Load {
            path: state.server.hls_path(job),
            content_type: HLS.into(),
            start,
            duration: plan.duration,
        }
    };
    let port = state.port;
    let base = format!("http://{}:{port}", cast::lan_ip(Some(device.ip)));
    let key = (device.ip, device.port);
    let previous = state.cast.lock().unwrap().take();
    let next_id = previous.as_ref().map_or(1, |p| p.id + 1);
    let media = match previous {
        // Same device: load the new file in the open session. If that session
        // has ended meanwhile, a new one starts below.
        Some(old) if old.device == key => {
            let (commands, url) = (old.commands.clone(), format!("{base}{}", media.path));
            let next = media.clone();
            let reloaded =
                tauri::async_runtime::spawn_blocking(move || cast::reload(&commands, url, next))
                    .await
                    .map_err(err)?;
            if reloaded.is_ok() {
                let id = old.id;
                *state.cast.lock().unwrap() = Some(old);
                return Ok(id);
            }
            media
        }
        // Another device: the old one closes its app first.
        Some(old) => {
            tauri::async_runtime::spawn_blocking(move || cast::stop(&old.commands))
                .await
                .map_err(err)?;
            media
        }
        None => media,
    };
    let (commands, inbox) = mpsc::channel();
    let session = next_id;
    *state.cast.lock().unwrap() = Some(CastSession {
        id: session,
        device: key,
        commands,
    });
    let started = tauri::async_runtime::spawn_blocking(move || {
        // Tagged so the window can drop late events of a replaced session.
        let on_event = move |event: cast::Event| {
            let _ = app.emit("cast", CastEvent { session, event });
        };
        cast::play(&device, &base, media, inbox, on_event)
    });
    let result = match within(30, "connecting to the device", started).await {
        Ok(Ok(Ok(()))) => Ok(session),
        Ok(Ok(Err(e))) => Err(err(e)),
        Ok(Err(e)) => Err(err(e)),
        Err(e) => Err(e),
    };
    if result.is_err() {
        state.cast.lock().unwrap().take_if(|s| s.id == session);
    }
    result
}

struct CastSession {
    id: u64,
    device: (std::net::IpAddr, u16),
    commands: mpsc::Sender<cast::Command>,
}

#[derive(Serialize, Clone)]
struct CastEvent {
    session: u64,
    #[serde(flatten)]
    event: cast::Event,
}

/// Remote control for the running Cast session. `time` carries the position
/// for `seek` and the level (0 to 1) for `volume`.
#[tauri::command]
async fn cast_control(
    action: String,
    time: Option<f64>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if action == "stop" {
        let current = state.cast.lock().unwrap().take();
        if let Some(session) = current {
            tauri::async_runtime::spawn_blocking(move || cast::stop(&session.commands))
                .await
                .map_err(err)?;
        }
        return Ok(());
    }
    let command = match action.as_str() {
        "play" => cast::Command::Play,
        "pause" => cast::Command::Pause,
        "seek" => cast::Command::Seek(time.filter(|t| t.is_finite()).ok_or("seek needs a time")?),
        "volume" => {
            let level = time
                .filter(|v| v.is_finite())
                .ok_or("volume needs a level")?;
            cast::Command::Volume(level.clamp(0.0, 1.0) as f32)
        }
        "mute" => cast::Command::Mute(true),
        "unmute" => cast::Command::Mute(false),
        other => return Err(format!("unknown action {other}")),
    };
    let cast = state.cast.lock().unwrap();
    let session = cast.as_ref().ok_or("not casting")?;
    session
        .commands
        .send(command)
        .map_err(|_| "cast session is over".to_string())
}

/// Shows or hides the macOS window buttons (close, minimise, zoom); the
/// title bar stays a transparent overlay. Does nothing on other systems.
#[tauri::command]
async fn window_buttons(window: tauri::WebviewWindow, visible: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Read off the main thread: `ns_window` itself waits for the main thread.
        let ptr = window.ns_window().map_err(err)? as usize;
        window
            .run_on_main_thread(move || {
                use objc2_app_kit::{NSWindow, NSWindowButton};
                // SAFETY: `ptr` is this window's live NSWindow, used on the main thread.
                let ns = unsafe { &*(ptr as *const NSWindow) };
                for kind in [
                    NSWindowButton::CloseButton,
                    NSWindowButton::MiniaturizeButton,
                    NSWindowButton::ZoomButton,
                ] {
                    if let Some(button) = ns.standardWindowButton(kind) {
                        button.setHidden(!visible);
                    }
                }
            })
            .map_err(err)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, visible);
    Ok(())
}

/// Queues files for the window and tells it to take them.
fn hand_over(app: &tauri::AppHandle, files: impl IntoIterator<Item = String>) {
    let files: Vec<String> = files.into_iter().collect();
    if files.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.incoming.lock().unwrap().extend(files);
    }
    let _ = app.emit("incoming", ());
}

/// Existing files among command-line arguments (Windows and Linux pass the
/// file to open that way).
fn file_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .skip(1)
        .filter(|a| PathBuf::from(a).is_file())
        .collect()
}

#[derive(Serialize)]
struct FfmpegAbout {
    version: String,
    licence: &'static str,
}

/// The ffmpeg in use, for the About popup; an error when there is none.
#[tauri::command]
fn ffmpeg_about(state: tauri::State<'_, AppState>) -> Result<FfmpegAbout, String> {
    let (version, licence) = state.transcoder()?.about.clone();
    Ok(FfmpegAbout { version, licence })
}

#[tauri::command]
fn take_incoming(state: tauri::State<'_, AppState>) -> Vec<String> {
    std::mem::take(&mut state.incoming.lock().unwrap())
}

/// The default macOS menu, with its first item (the system About panel)
/// replaced by one that asks the window to show its own About popup.
#[cfg(target_os = "macos")]
fn about_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app.handle())?;
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
        let label = format!("About {}", app.package_info().name);
        app_menu.remove_at(0)?;
        app_menu.insert(
            &MenuItem::with_id(app, "about", label, true, None::<&str>)?,
            0,
        )?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id() == "about" {
            let _ = app.emit("about", ());
        }
    });
    Ok(())
}

#[tauri::command]
fn shield_state(state: tauri::State<'_, AppState>) -> shield::Shield {
    state.shield.clone()
}

/// Saves the shield. It takes effect at the next launch, since the torrent
/// session is built from it.
#[tauri::command]
fn set_shield(state: tauri::State<'_, AppState>, on: bool, proxy: String) -> Result<(), String> {
    let proxy = proxy.trim().to_owned();
    if on {
        shield::check_proxy(&proxy)?;
    }
    shield::Shield { on, proxy }
        .save(&state.shield_file)
        .map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Windows and Linux start a second process for a magnet link; it hands the
    // link to the running app (through the deep-link plugin) and quits.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _| {
        hand_over(app, file_args(args));
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_focus();
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Linux has no install step that registers the magnet scheme.
            #[cfg(target_os = "linux")]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register_all();
            }
            #[cfg(target_os = "macos")]
            about_menu(app)?;
            let scratch = scratch_dir();
            let _ = std::fs::remove_dir_all(&scratch);
            let transcoder = transcode::Transcoder::new(scratch.join("hls")).map_err(err);
            let shield_file = app.path().app_config_dir()?.join("shield.json");
            let shield = shield::Shield::load(&shield_file);
            let opts = shield::session_opts(&shield);
            let state = tauri::async_runtime::block_on(async {
                let session = Session::new_with_opts(scratch.join("downloads"), opts).await?;
                let (server, port) =
                    server::start(session.clone(), transcoder.clone().ok()).await?;
                anyhow::Ok(AppState {
                    api: librqbit::Api::new(session.clone(), None),
                    session,
                    server,
                    transcoder,
                    cast: Mutex::default(),
                    selected: Mutex::default(),
                    probing: tokio::sync::Mutex::default(),
                    incoming: Mutex::default(),
                    port,
                    shield,
                    shield_file,
                })
            })?;
            app.manage(state);
            hand_over(app.handle(), file_args(std::env::args()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open,
            open_episode,
            episode_info,
            file_info,
            torrent_stats,
            discover_devices,
            cast_to,
            cast_control,
            window_buttons,
            take_incoming,
            ffmpeg_about,
            shield_state,
            set_shield
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS hands files over as events (Open With, a double click).
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                let files = urls
                    .iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .map(|p| p.to_string_lossy().into_owned());
                hand_over(app, files);
            }
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                // Leave the TV idle rather than playing a stream that is gone.
                if let Some(session) = state.cast.lock().unwrap().take() {
                    cast::stop(&session.commands);
                }
                // The process exits without dropping state, so ffmpeg is stopped here.
                if let Ok(t) = state.transcoder() {
                    tauri::async_runtime::block_on(t.shutdown());
                }
                let _ = std::fs::remove_dir_all(scratch_dir());
            }
        });
}
