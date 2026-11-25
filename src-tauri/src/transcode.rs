//! Turns any source the stream server can serve into HLS (H.264 + AAC in
//! MPEG-TS segments): the one format the webview, iOS Safari and the Cast
//! Default Media Receiver all play.
//!
//! The playlist is a complete VOD list of fixed-length segments written up
//! front from the duration, so players can seek anywhere. Segments are made on
//! demand: a request for one that does not exist restarts ffmpeg at that
//! segment. Video is always encoded with keyframes forced on segment
//! boundaries and timestamps offset to the segment start, so segments from
//! different ffmpeg runs line up. Only a window of segments around the last
//! request is kept on disk, and ffmpeg is stopped when it runs too far ahead.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use crate::media::{self, Stereo, Vr};
use anyhow::{Context, bail};
use serde::Deserialize;
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

/// Segment length in seconds.
pub const SEG: f64 = 4.0;
/// Segments kept and produced ahead of the last request (one minute).
const AHEAD: u32 = 15;
/// Segments kept behind the last request (one minute).
const BEHIND: u32 = 15;

#[derive(Deserialize, Debug, Default)]
pub struct Probe {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub chapters: Vec<RawChapter>,
}

#[derive(Deserialize, Debug, Default)]
pub struct RawChapter {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

#[derive(serde::Serialize, Debug, PartialEq, Clone)]
pub struct Chapter {
    pub start: f64,
    pub end: f64,
    pub title: String,
}

/// Chapters with usable times, in order; untitled ones are numbered.
pub fn chapters(probe: &Probe) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = probe
        .chapters
        .iter()
        .filter_map(|c| {
            let start: f64 = c.start_time.as_deref()?.parse().ok()?;
            let end: f64 = c.end_time.as_deref()?.parse().ok()?;
            let title = c
                .tags
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("title"))
                .map(|(_, v)| v.trim().to_string());
            (end > start).then(|| Chapter {
                start,
                end,
                title: title.unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    for (i, c) in out.iter_mut().enumerate() {
        if c.title.is_empty() {
            c.title = format!("Chapter {}", i + 1);
        }
    }
    out
}

#[derive(Deserialize, Debug, Default)]
pub struct Format {
    pub format_name: Option<String>,
    pub duration: Option<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub start_time: Option<String>,
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

impl Format {
    /// The container's title tag; its case differs between MKV and MP4.
    pub fn title(&self) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, v)| k.eq_ignore_ascii_case("title") && !v.trim().is_empty())
            .map(|(_, v)| v.trim())
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct Stream {
    pub index: u32,
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i32>,
    pub pix_fmt: Option<String>,
    pub channels: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    #[serde(default)]
    pub disposition: Disposition,
    #[serde(default)]
    pub side_data_list: Vec<SideData>,
}

/// Per-stream side data; only the VR related kinds are read.
#[derive(Deserialize, Debug, Default, Clone)]
pub struct SideData {
    #[serde(default)]
    pub side_data_type: String,
    /// The packing, for "Stereo 3D" ("side by side", "top and bottom").
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct Disposition {
    #[serde(default)]
    pub default: u8,
}

/// What to do with one input stream.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Track {
    pub index: u32,
    pub copy: bool,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Plan {
    pub video: Track,
    pub audio: Option<Track>,
    pub duration: Option<f64>,
    /// The source is an MP4 a Cast device can play as it is.
    pub direct: bool,
    /// Reproject this VR video to a flat, centred view (for the Cast device).
    pub flat: Option<Vr>,
    /// Constant frame rate to convert a variable-rate video to.
    pub fps: Option<String>,
}

/// VR layout of the main video stream, from its side data and `name`.
pub fn vr(probe: &Probe, name: &str) -> Option<Vr> {
    let video = probe.streams.iter().find(|s| s.codec_type == "video")?;
    let side = |kind: &str| {
        video
            .side_data_list
            .iter()
            .find(|d| d.side_data_type.contains(kind))
    };
    let spherical = side("Spherical").is_some();
    let stereo = side("Stereo 3D").and_then(|d| d.kind.as_deref());
    media::detect_vr(
        name,
        video.width.unwrap_or(0),
        video.height.unwrap_or(0),
        spherical,
        stereo,
    )
}

/// `v360` view for the Cast device: straight ahead, 100 degrees wide, 16:9.
fn flat_view(vr: Vr) -> String {
    let input = if vr.fov == 180 { "hequirect" } else { "e" };
    let stereo = match vr.stereo {
        Stereo::Mono => "2d",
        Stereo::Sbs => "sbs",
        Stereo::Tb => "tb",
    };
    format!(
        "v360=input={input}:output=flat:in_stereo={stereo}:out_stereo=2d:h_fov=100:v_fov=67.7:w=1920:h=1080"
    )
}

/// H.264 that every Cast device and iOS decode: 8-bit 4:2:0, up to level 4.1.
fn video_fits(s: &Stream) -> bool {
    let profile_ok = matches!(
        s.profile.as_deref(),
        Some("Baseline" | "Constrained Baseline" | "Main" | "High")
    );
    s.codec_name.as_deref() == Some("h264")
        && profile_ok
        && matches!(s.pix_fmt.as_deref(), Some("yuv420p" | "yuvj420p"))
        && s.level.is_some_and(|l| l <= 41)
}

/// Stereo AAC: multichannel AAC is not decoded by every Cast device.
fn audio_fits(s: &Stream) -> bool {
    s.codec_name.as_deref() == Some("aac") && s.channels.is_some_and(|c| c <= 2)
}

pub fn plan(probe: &Probe) -> anyhow::Result<Plan> {
    let first = |kind: &str| {
        let mut of_kind = probe.streams.iter().filter(move |s| s.codec_type == kind);
        let default = of_kind.clone().find(|s| s.disposition.default == 1);
        default.or_else(|| of_kind.next())
    };
    let video = first("video").context("no video stream")?;
    let audio = first("audio");
    let video = Track {
        index: video.index,
        copy: video_fits(video),
    };
    let audio = audio.map(|a| Track {
        index: a.index,
        copy: audio_fits(a),
    });
    let is_mp4 = probe
        .format
        .format_name
        .as_deref()
        .is_some_and(|f| f.contains("mp4"));
    let fps = probe
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .and_then(variable_rate);
    Ok(Plan {
        direct: is_mp4 && video.copy && audio.is_none_or(|a| a.copy) && fps.is_none(),
        video,
        audio,
        duration: probe
            .format
            .duration
            .as_deref()
            .and_then(|d| d.parse().ok()),
        flat: None,
        fps,
    })
}

/// The average frame rate of a video whose frames are not evenly spaced
/// (screen and game recordings): its nominal rate is more than 5% off the
/// average. WebKit plays such files with visible stutter.
fn variable_rate(stream: &Stream) -> Option<String> {
    let rate = |r: &Option<String>| {
        let (n, d) = r.as_deref()?.split_once('/')?;
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        (n > 0.0 && d > 0.0).then(|| n / d)
    };
    let (nominal, average) = (rate(&stream.r_frame_rate)?, rate(&stream.avg_frame_rate)?);
    ((nominal - average).abs() > average * 0.05)
        .then(|| stream.avg_frame_rate.clone().unwrap_or_default())
}

/// Codecs WebKit on macOS decodes from an MP4, WebM or QuickTime file.
pub fn webview_plays(probe: &Probe) -> bool {
    let codec = |kind: &str| {
        probe
            .streams
            .iter()
            .find(|s| s.codec_type == kind)
            .and_then(|s| s.codec_name.as_deref())
    };
    let video = matches!(
        codec("video"),
        Some("h264" | "hevc" | "vp8" | "vp9" | "av1")
    );
    let audio = matches!(
        codec("audio"),
        None | Some("aac" | "mp3" | "opus" | "vorbis" | "flac" | "alac")
    );
    let steady = probe
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .and_then(variable_rate)
        .is_none();
    video && audio && steady
}

pub fn segment_count(duration: f64) -> u32 {
    (duration / SEG).ceil().max(1.0) as u32
}

pub fn segment_name(index: u32) -> String {
    format!("seg{index:05}.ts")
}

/// Complete VOD playlist of `SEG`-second segments covering `duration`.
pub fn playlist(duration: f64) -> String {
    let count = segment_count(duration);
    let mut p = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        SEG as u32 + 1
    );
    for i in 0..count {
        let len = (duration - i as f64 * SEG).min(SEG);
        p.push_str(&format!("#EXTINF:{len:.6},\n{}\n", segment_name(i)));
    }
    p.push_str("#EXT-X-ENDLIST\n");
    p
}

/// A file a job directory may serve; anything else is refused.
#[derive(Debug, PartialEq)]
pub enum HlsFile {
    Playlist,
    Segment(u32),
}

pub fn parse_hls_name(name: &str) -> Option<HlsFile> {
    if name == "index.m3u8" {
        return Some(HlsFile::Playlist);
    }
    let digits = name.strip_prefix("seg")?.strip_suffix(".ts")?;
    if digits.len() != 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(HlsFile::Segment)
}

/// ffmpeg arguments that write segments from `start` (a segment index) on,
/// named and timed as in `playlist`. Streams are mapped explicitly so MKV
/// subtitles and extra audio tracks never reach the TS muxer.
pub fn ffmpeg_args(plan: &Plan, input: &str, start: u32, out: &Path, hw: bool) -> Vec<String> {
    let offset = format!("{:.3}", start as f64 * SEG);
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .map(String::from)
        .into();
    if start > 0 {
        a.extend(["-ss".into(), offset.clone()]);
    }
    // Decoding on the GPU too; ffmpeg falls back to software for a codec the
    // GPU cannot decode, and copies frames back for the filters.
    if hw {
        a.extend(["-hwaccel".into(), "videotoolbox".into()]);
    }
    a.extend(["-i".into(), input.into()]);
    a.extend(["-map".into(), format!("0:{}", plan.video.index)]);
    if let Some(audio) = plan.audio {
        a.extend(["-map".into(), format!("0:{}", audio.index)]);
    }
    let encoder: &[&str] = if hw {
        &["-c:v", "h264_videotoolbox", "-b:v", "8M", "-allow_sw", "1"]
    } else {
        &["-c:v", "libx264", "-preset", "veryfast", "-crf", "21"]
    };
    a.extend(encoder.iter().map(|s| s.to_string()));
    a.extend(["-profile:v", "high", "-level", "4.1", "-pix_fmt", "yuv420p"].map(String::from));
    let filter = plan
        .flat
        .map_or_else(|| "scale=w='min(1920,iw)':h=-2".to_string(), flat_view);
    a.extend(["-vf".into(), filter]);
    if let Some(fps) = &plan.fps {
        a.extend(["-fps_mode".into(), "cfr".into(), "-r".into(), fps.clone()]);
    }
    a.extend([
        "-force_key_frames".into(),
        format!("expr:gte(t,n_forced*{SEG})"),
    ]);
    if let Some(audio) = plan.audio {
        if audio.copy {
            a.extend(["-c:a", "copy"].map(String::from));
        } else {
            a.extend(["-c:a", "aac", "-ac", "2", "-b:a", "192k"].map(String::from));
        }
    }
    a.extend(["-output_ts_offset".into(), offset]);
    a.extend(
        [
            "-f",
            "hls",
            "-hls_list_size",
            "0",
            "-hls_flags",
            "temp_file",
        ]
        .map(String::from),
    );
    a.extend(["-hls_time".into(), SEG.to_string()]);
    a.extend([
        "-start_number".into(),
        start.to_string(),
        "-hls_segment_filename".into(),
        out.join("seg%05d.ts").to_string_lossy().into(),
        // ffmpeg's own playlist is never served: the name is not accepted.
        out.join("ffmpeg.m3u8").to_string_lossy().into(),
    ]);
    a
}

/// Whether ffmpeg must (re)start for `wanted`: it is not running, started
/// after `wanted`, or is too far behind it to arrive soon. `present` are the
/// segments on disk; only those from `run_start` on show how far this run got.
pub fn needs_restart(run_start: Option<u32>, present: &[u32], wanted: u32) -> bool {
    match run_start {
        None => true,
        Some(start) => {
            let reached = present
                .iter()
                .copied()
                .filter(|&n| n >= start)
                .max()
                .unwrap_or(start);
            wanted < start || wanted > reached + 2
        }
    }
}

/// Segments to delete when `last` was the latest request.
pub fn outside_window(present: &[u32], last: u32) -> Vec<u32> {
    let low = last.saturating_sub(BEHIND);
    let high = last.saturating_add(AHEAD + 2);
    present
        .iter()
        .copied()
        .filter(|&i| i < low || i > high)
        .collect()
}

struct Running {
    child: Child,
    start: u32,
}

struct Job {
    id: u64,
    input: String,
    plan: Plan,
    dir: PathBuf,
    ffmpeg: Option<Running>,
    last_request: u32,
}

impl Job {
    fn present(&self) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        entries
            .filter_map(|e| match parse_hls_name(e.ok()?.file_name().to_str()?) {
                Some(HlsFile::Segment(i)) => Some(i),
                _ => None,
            })
            .collect()
    }

    fn spawn(&mut self, start: u32, ffmpeg: &Path, hw: bool) -> anyhow::Result<()> {
        // Dropping the old child kills it (kill_on_drop).
        self.ffmpeg = None;
        let log = std::fs::File::create(self.dir.join("ffmpeg.log"))?;
        let child = tool(ffmpeg)
            .args(ffmpeg_args(&self.plan, &self.input, start, &self.dir, hw))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .kill_on_drop(true)
            .spawn()?;
        self.ffmpeg = Some(Running { child, start });
        Ok(())
    }

    /// Stops ffmpeg once it is far ahead of playback and drops segments
    /// outside the window. Returns whether an ffmpeg run is still kept.
    fn tidy(&mut self) -> bool {
        let present = self.present();
        for i in outside_window(&present, self.last_request) {
            let _ = std::fs::remove_file(self.dir.join(segment_name(i)));
        }
        let Some(run) = &self.ffmpeg else {
            return false;
        };
        let newest = present.iter().copied().filter(|&i| i >= run.start).max();
        // An ffmpeg that already exited is left for `segment` to inspect.
        if newest.is_some_and(|n| n > self.last_request + AHEAD) {
            self.ffmpeg = None;
        }
        self.ffmpeg.is_some()
    }
}

/// Runs the jobs of one file, at most one per variant, shared by every player
/// of that file. Opening another file kills their ffmpeg and deletes their
/// segments.
pub struct Transcoder {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    hw: bool,
    /// Version and licence of the ffmpeg in use, for the About popup.
    pub about: (String, &'static str),
    root: PathBuf,
    jobs: Mutex<Jobs>,
    thumbs: tokio::sync::Semaphore,
}

/// The running jobs: at most one per variant (plain, flat VR view), all for
/// the same file.
#[derive(Default)]
struct Jobs {
    last_id: u64,
    list: Vec<Job>,
}

impl Jobs {
    fn get(&mut self, id: u64) -> Option<&mut Job> {
        self.list.iter_mut().find(|j| j.id == id)
    }
}

/// FFmpeg tools run without a console window, which Windows would otherwise
/// open for each process of a GUI app.
fn tool(path: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(path);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn tool_sync(path: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Finds a tool in `beside` (the app's own directory, where the bundled
/// FFmpeg sits), then in PATH or the usual install directories, since an app
/// started from Finder does not get the shell's PATH.
fn find_tool(name: &str, beside: Option<&Path>) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    beside
        .map(Path::to_path_buf)
        .into_iter()
        .chain(std::env::split_paths(&path))
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|p| p.is_file())
}

/// Version and SPDX licence of an ffmpeg build from its `-version` output.
/// The builder's `--extra-version` tag is cut off the version; `--enable-gpl`
/// makes the build GPL, `--enable-version3` makes it version 3.
fn ffmpeg_about(version: &str) -> (String, &'static str) {
    let config = version
        .lines()
        .find(|l| l.starts_with("configuration:"))
        .unwrap_or_default();
    let flag = |f: &str| config.split_whitespace().any(|w| w == f);
    let extra = config
        .split_whitespace()
        .find_map(|w| w.strip_prefix("--extra-version="))
        .map(|v| format!("-{}", v.trim_matches(['\'', '"'])));
    let number = version.split_whitespace().nth(2).unwrap_or_default();
    let number = extra
        .and_then(|e| number.strip_suffix(&e))
        .unwrap_or(number)
        .to_string();
    let licence = match (flag("--enable-gpl"), flag("--enable-version3")) {
        (true, true) => "GPL-3.0-or-later",
        (true, false) => "GPL-2.0-or-later",
        (false, true) => "LGPL-3.0-or-later",
        (false, false) => "LGPL-2.1-or-later",
    };
    (number, licence)
}

impl Transcoder {
    pub fn new(root: PathBuf) -> anyhow::Result<Arc<Self>> {
        let exe = std::env::current_exe().ok();
        let beside = exe.as_deref().and_then(Path::parent);
        let ffmpeg = find_tool("ffmpeg", beside).context("ffmpeg not found")?;
        let ffprobe = find_tool("ffprobe", beside).context("ffprobe not found")?;
        let hw = tool_sync(&ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("h264_videotoolbox"));
        let version = tool_sync(&ffmpeg).arg("-version").output()?;
        let about = ffmpeg_about(&String::from_utf8_lossy(&version.stdout));
        // Segments of a previous run are never reused.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root)?;
        Ok(Arc::new(Self {
            ffmpeg,
            ffprobe,
            hw,
            about,
            root,
            jobs: Mutex::default(),
            thumbs: tokio::sync::Semaphore::new(2),
        }))
    }

    pub async fn probe(&self, input: &str) -> anyhow::Result<Probe> {
        let out = tool(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-of",
                "json",
                "-show_format",
                "-show_streams",
                "-show_chapters",
                input,
            ])
            .kill_on_drop(true)
            .output()
            .await?;
        if !out.status.success() {
            bail!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    /// A JPEG of the frame at `at` seconds, 320 px wide, or `None` when two
    /// are already being made. Gives up after 15 s (a torrent may not have
    /// the data yet).
    pub async fn thumbnail(&self, input: &str, at: f64) -> anyhow::Result<Option<Vec<u8>>> {
        let Ok(_slot) = self.thumbs.try_acquire() else {
            return Ok(None);
        };
        let run = tool(&self.ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-ss",
                &format!("{:.3}", at.max(0.0)),
                "-i",
                input,
            ])
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=320:-2",
                "-f",
                "image2",
                "-c:v",
                "mjpeg",
                "-q:v",
                "5",
                "pipe:1",
            ])
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(Duration::from_secs(15), run)
            .await
            .context("thumbnail timed out")??;
        if !out.status.success() || out.stdout.is_empty() {
            bail!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(Some(out.stdout))
    }

    #[cfg(test)]
    pub fn ffmpeg_path(&self) -> &Path {
        &self.ffmpeg
    }

    /// Stops every ffmpeg and deletes the segments.
    pub async fn shutdown(&self) {
        let mut jobs = self.jobs.lock().await;
        for mut job in jobs.list.drain(..) {
            if let Some(mut run) = job.ffmpeg.take() {
                let _ = run.child.kill().await;
            }
            let _ = std::fs::remove_dir_all(&job.dir);
        }
    }

    /// Returns the job for `input` and the plan's variant, creating it if
    /// needed; jobs for other files are dropped with their segments. The
    /// playlist exists when this returns; segments come on demand.
    pub async fn open(self: &Arc<Self>, input: &str, plan: &Plan) -> anyhow::Result<u64> {
        let duration = plan
            .duration
            .context("unknown duration, cannot build a playlist")?;
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs
            .list
            .iter()
            .find(|j| j.input == input && j.plan.flat == plan.flat)
        {
            return Ok(job.id);
        }
        let (keep, gone): (Vec<Job>, Vec<Job>) =
            jobs.list.drain(..).partition(|j| j.input == input);
        jobs.list = keep;
        for mut old in gone {
            // ffmpeg must be gone before its directory, or it writes into it again.
            if let Some(mut run) = old.ffmpeg.take() {
                let _ = run.child.kill().await;
            }
            let _ = std::fs::remove_dir_all(&old.dir);
        }
        jobs.last_id += 1;
        let id = jobs.last_id;
        let dir = self.root.join(id.to_string());
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("index.m3u8"), playlist(duration))?;
        let mut job = Job {
            id,
            input: input.into(),
            plan: plan.clone(),
            dir,
            ffmpeg: None,
            last_request: 0,
        };
        job.spawn(0, &self.ffmpeg, self.hw)?;
        jobs.list.push(job);
        drop(jobs);
        tokio::spawn(watch(Arc::downgrade(self), id));
        Ok(id)
    }

    pub async fn playlist_path(&self, id: u64) -> Option<PathBuf> {
        let mut jobs = self.jobs.lock().await;
        jobs.get(id).map(|j| j.dir.join("index.m3u8"))
    }

    /// Path of segment `index` of job `id`, waiting until ffmpeg wrote it.
    pub async fn segment(&self, id: u64, index: u32) -> anyhow::Result<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            {
                let mut jobs = self.jobs.lock().await;
                let job = jobs.get(id).context("job replaced")?;
                // Out of range requests must not move playback or kill ffmpeg.
                if index >= segment_count(job.plan.duration.unwrap_or(0.0)) {
                    bail!("no segment {index}");
                }
                job.last_request = index;
                let path = job.dir.join(segment_name(index));
                if path.is_file() {
                    return Ok(path);
                }
                let run_start = job.ffmpeg.as_ref().map(|r| r.start);
                let present = job.present();
                if let Some(run) = job.ffmpeg.as_mut()
                    && let Some(status) = run.child.try_wait()?
                {
                    // The run started for this very segment ended without it.
                    if run.start == index {
                        let log =
                            std::fs::read_to_string(job.dir.join("ffmpeg.log")).unwrap_or_default();
                        bail!(
                            "ffmpeg ({status}) did not produce segment {index}: {}",
                            log.trim()
                        );
                    }
                    job.ffmpeg = None;
                }
                if job.ffmpeg.is_none() || needs_restart(run_start, &present, index) {
                    job.spawn(index, &self.ffmpeg, self.hw)?;
                }
            }
            if Instant::now() > deadline {
                bail!("segment {index} not ready within 60 s");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Keeps a job tidy until it is replaced or the transcoder is gone.
async fn watch(transcoder: Weak<Transcoder>, id: u64) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(t) = transcoder.upgrade() else {
            return;
        };
        let mut jobs = t.jobs.lock().await;
        match jobs.get(id) {
            Some(job) => {
                job.tidy();
            }
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(codec: &str, profile: &str, pix: &str, level: i32) -> Stream {
        Stream {
            index: 0,
            codec_type: "video".into(),
            codec_name: Some(codec.into()),
            profile: Some(profile.into()),
            pix_fmt: Some(pix.into()),
            level: Some(level),
            ..Default::default()
        }
    }

    fn audio(index: u32, codec: &str, channels: u32, default: u8) -> Stream {
        Stream {
            index,
            codec_type: "audio".into(),
            codec_name: Some(codec.into()),
            channels: Some(channels),
            disposition: Disposition { default },
            ..Default::default()
        }
    }

    fn probe(format: &str, streams: Vec<Stream>) -> Probe {
        Probe {
            streams,
            format: Format {
                format_name: Some(format.into()),
                duration: Some("61.5".into()),
                start_time: None,
                tags: Default::default(),
            },
            chapters: vec![],
        }
    }

    const MP4: &str = "mov,mp4,m4a,3gp,3g2,mj2";
    const MKV: &str = "matroska,webm";

    #[test]
    fn title_tag_in_any_case() {
        let parsed: Probe =
            serde_json::from_str(r#"{"format":{"tags":{"TITLE":" Pilot "}}}"#).unwrap();
        assert_eq!(parsed.format.title(), Some("Pilot"));
        let lower: Probe = serde_json::from_str(r#"{"format":{"tags":{"title":"Two"}}}"#).unwrap();
        assert_eq!(lower.format.title(), Some("Two"));
        let blank: Probe = serde_json::from_str(r#"{"format":{"tags":{"title":"  "}}}"#).unwrap();
        assert_eq!(blank.format.title(), None);
        assert_eq!(Probe::default().format.title(), None);
    }

    fn rates(mut s: Stream, nominal: &str, average: &str) -> Stream {
        s.r_frame_rate = Some(nominal.into());
        s.avg_frame_rate = Some(average.into());
        s
    }

    #[test]
    fn variable_frame_rate_goes_through_conversion_at_a_constant_rate() {
        let screen = rates(video("h264", "Main", "yuv420p", 42), "180/1", "6000/101");
        let p = probe(MP4, vec![screen, audio(1, "aac", 2, 1)]);
        assert!(!webview_plays(&p));
        assert_eq!(plan(&p).unwrap().fps.as_deref(), Some("6000/101"));
        assert!(
            !plan(&p).unwrap().direct,
            "a Cast device gets it converted too"
        );
        let a = ffmpeg_args(&plan(&p).unwrap(), "in", 0, Path::new("/o"), false);
        assert!(has(&a, ["-fps_mode", "cfr"]) && has(&a, ["-r", "6000/101"]));

        for (nominal, average) in [
            ("30000/1001", "30000/1001"),
            ("30/1", "2997/100"),
            ("25/1", "0/0"),
            ("60/1", ""),
        ] {
            let steady = probe(
                MP4,
                vec![rates(
                    video("h264", "High", "yuv420p", 41),
                    nominal,
                    average,
                )],
            );
            assert!(webview_plays(&steady), "{nominal} {average}");
            assert_eq!(plan(&steady).unwrap().fps, None, "{nominal} {average}");
            assert!(plan(&steady).unwrap().direct, "{nominal} {average}");
            assert!(
                !ffmpeg_args(&plan(&steady).unwrap(), "in", 0, Path::new("/o"), false)
                    .contains(&"-fps_mode".to_string())
            );
        }
    }

    #[test]
    fn compatible_mp4_plays_directly() {
        let p = plan(&probe(
            MP4,
            vec![video("h264", "High", "yuv420p", 41), audio(1, "aac", 2, 1)],
        ))
        .unwrap();
        assert!(p.direct);
        assert_eq!(
            p.video,
            Track {
                index: 0,
                copy: true
            }
        );
        assert_eq!(
            p.audio,
            Some(Track {
                index: 1,
                copy: true
            })
        );
        assert_eq!(p.duration, Some(61.5));
    }

    #[test]
    fn compatible_mkv_is_not_direct() {
        let p = plan(&probe(
            MKV,
            vec![video("h264", "Main", "yuv420p", 40), audio(1, "aac", 2, 0)],
        ))
        .unwrap();
        assert!(!p.direct);
        assert!(p.video.copy && p.audio.unwrap().copy);
    }

    #[test]
    fn video_outside_cast_profile_is_not_direct() {
        for v in [
            video("hevc", "Main 10", "yuv420p10le", 120),
            video("h264", "High 10", "yuv420p10le", 41),
            video("h264", "High", "yuv420p", 51),
            video("h264", "High 4:4:4 Predictive", "yuv444p", 41),
        ] {
            let p = plan(&probe(MP4, vec![v.clone(), audio(1, "aac", 2, 0)])).unwrap();
            assert!(!p.video.copy, "{v:?}");
            assert!(!p.direct);
        }
    }

    #[test]
    fn audio_outside_cast_profile_is_transcoded() {
        for a in [
            audio(1, "ac3", 6, 0),
            audio(1, "dts", 6, 0),
            audio(1, "aac", 6, 0),
        ] {
            let p = plan(&probe(MP4, vec![video("h264", "High", "yuv420p", 41), a])).unwrap();
            assert!(!p.audio.unwrap().copy);
            assert!(!p.direct);
        }
    }

    #[test]
    fn default_audio_track_wins_over_first() {
        let p = plan(&probe(
            MKV,
            vec![
                video("h264", "High", "yuv420p", 41),
                audio(1, "ac3", 6, 0),
                audio(2, "aac", 2, 1),
            ],
        ))
        .unwrap();
        assert_eq!(
            p.audio,
            Some(Track {
                index: 2,
                copy: true
            })
        );
    }

    #[test]
    fn silent_video_is_fine_and_no_video_is_an_error() {
        let p = plan(&probe(MP4, vec![video("h264", "High", "yuv420p", 41)])).unwrap();
        assert_eq!(p.audio, None);
        assert!(p.direct);
        assert!(plan(&probe(MP4, vec![audio(0, "aac", 2, 1)])).is_err());
    }

    #[test]
    fn playlist_covers_duration_with_fixed_segments() {
        let p = playlist(10.5);
        assert!(p.starts_with("#EXTM3U\n"));
        assert!(p.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
        assert!(p.ends_with("#EXT-X-ENDLIST\n"));
        let extinf: Vec<f64> = p
            .lines()
            .filter_map(|l| l.strip_prefix("#EXTINF:")?.strip_suffix(',')?.parse().ok())
            .collect();
        assert_eq!(extinf, vec![4.0, 4.0, 2.5]);
        assert!(p.contains("seg00000.ts\n") && p.contains("seg00002.ts\n"));
        assert!(!p.contains("seg00003.ts"));
        assert_eq!(segment_count(8.0), 2);
        assert_eq!(segment_count(8.001), 3);
        assert_eq!(segment_count(0.0), 1);
    }

    fn has(args: &[String], pair: [&str; 2]) -> bool {
        args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1])
    }

    fn plan_of(audio: Option<bool>) -> Plan {
        Plan {
            video: Track {
                index: 0,
                copy: true,
            },
            audio: audio.map(|copy| Track { index: 2, copy }),
            duration: Some(60.0),
            direct: false,
            flat: None,
            fps: None,
        }
    }

    #[test]
    fn args_from_start_encode_video_and_align_segments() {
        let a = ffmpeg_args(
            &plan_of(Some(true)),
            "http://x/in",
            0,
            Path::new("/o"),
            true,
        );
        assert!(has(&a, ["-map", "0:0"]) && has(&a, ["-map", "0:2"]));
        assert!(has(&a, ["-c:v", "h264_videotoolbox"]) && has(&a, ["-pix_fmt", "yuv420p"]));
        assert!(has(&a, ["-hwaccel", "videotoolbox"]));
        assert!(a.iter().position(|x| x == "-hwaccel") < a.iter().position(|x| x == "-i"));
        assert!(has(&a, ["-c:a", "copy"]));
        assert!(has(&a, ["-force_key_frames", "expr:gte(t,n_forced*4)"]));
        assert!(has(&a, ["-hls_time", "4"]));
        assert!(!a.contains(&"-ss".to_string()));
        assert!(has(&a, ["-output_ts_offset", "0.000"]));
        assert!(has(&a, ["-start_number", "0"]));
        assert!(has(&a, ["-i", "http://x/in"]));
        assert!(has(&a, ["-hls_segment_filename", "/o/seg%05d.ts"]));
        assert_eq!(a.last().unwrap(), "/o/ffmpeg.m3u8");
    }

    #[test]
    fn args_restart_mid_file() {
        let a = ffmpeg_args(&plan_of(Some(false)), "in", 30, Path::new("/o"), false);
        assert!(has(&a, ["-ss", "120.000"]));
        assert!(a.iter().position(|x| x == "-ss") < a.iter().position(|x| x == "-i"));
        assert!(has(&a, ["-output_ts_offset", "120.000"]));
        assert!(has(&a, ["-start_number", "30"]));
        assert!(has(&a, ["-c:v", "libx264"]));
        assert!(!a.contains(&"-hwaccel".to_string()));
        assert!(has(&a, ["-c:a", "aac"]) && has(&a, ["-ac", "2"]));
        let silent = ffmpeg_args(&plan_of(None), "in", 0, Path::new("/o"), false);
        assert!(!silent.iter().any(|x| x.starts_with("-c:a")));
        assert_eq!(silent.iter().filter(|x| *x == "-map").count(), 1);
    }

    #[test]
    fn flat_vr_view_uses_v360() {
        let mut p = plan_of(None);
        p.flat = Some(Vr {
            fov: 180,
            stereo: Stereo::Sbs,
        });
        let a = ffmpeg_args(&p, "in", 0, Path::new("/o"), true);
        let vf = &a[a.iter().position(|x| x == "-vf").unwrap() + 1];
        assert!(
            vf.starts_with("v360=input=hequirect:output=flat:in_stereo=sbs:out_stereo=2d"),
            "{vf}"
        );
        assert!(vf.contains("w=1920:h=1080"));
        p.flat = Some(Vr {
            fov: 360,
            stereo: Stereo::Tb,
        });
        let a = ffmpeg_args(&p, "in", 0, Path::new("/o"), true);
        assert!(
            a.iter()
                .any(|x| x.starts_with("v360=input=e:output=flat:in_stereo=tb"))
        );
        p.flat = Some(Vr {
            fov: 360,
            stereo: Stereo::Mono,
        });
        let a = ffmpeg_args(&p, "in", 0, Path::new("/o"), true);
        assert!(a.iter().any(|x| x.contains("in_stereo=2d")));
        let plain = ffmpeg_args(&plan_of(None), "in", 0, Path::new("/o"), true);
        assert!(has(&plain, ["-vf", "scale=w='min(1920,iw)':h=-2"]));
    }

    #[test]
    fn vr_read_from_side_data() {
        let json = r#"{"streams":[{"index":0,"codec_type":"video","width":4096,"height":2048,
            "side_data_list":[{"side_data_type":"Spherical Mapping","projection":"equirectangular"},
                              {"side_data_type":"Stereo 3D","type":"side by side"}]}]}"#;
        let probe: Probe = serde_json::from_str(json).unwrap();
        assert_eq!(
            vr(&probe, "clip.mp4"),
            Some(Vr {
                fov: 180,
                stereo: Stereo::Sbs
            })
        );
        let plain: Probe = serde_json::from_str(
            r#"{"streams":[{"index":0,"codec_type":"video","width":1920,"height":1080}]}"#,
        )
        .unwrap();
        assert_eq!(vr(&plain, "clip.mp4"), None);
        assert_eq!(
            vr(&plain, "clip_360.mp4"),
            Some(Vr {
                fov: 360,
                stereo: Stereo::Mono
            })
        );
        assert_eq!(vr(&Probe::default(), "clip_360.mp4"), None);
    }

    /// A plain and a flat VR job run side by side for one file; another file
    /// drops both.
    #[tokio::test]
    async fn plain_and_flat_jobs_share_a_file() {
        let root = std::env::temp_dir().join(format!("tonic-vr-{}", std::process::id()));
        let Ok(t) = Transcoder::new(root.clone()) else {
            return;
        };
        let src = root.join("trip_360.mkv");
        let made = std::process::Command::new(&t.ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x320:rate=25:duration=5",
            ])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let input = src.to_string_lossy().to_string();
        let probe = t.probe(&input).await.unwrap();
        let layout = vr(&probe, "trip_360.mkv");
        assert_eq!(
            layout,
            Some(Vr {
                fov: 360,
                stereo: Stereo::Mono
            })
        );
        let plain = plan(&probe).unwrap();
        let flat = Plan {
            flat: layout,
            ..plain.clone()
        };

        let a = t.open(&input, &plain).await.unwrap();
        let b = t.open(&input, &flat).await.unwrap();
        assert_ne!(a, b);
        assert_eq!(t.open(&input, &plain).await.unwrap(), a, "plain job kept");
        let seg = t.segment(b, 0).await.unwrap();
        let out = t.probe(&seg.to_string_lossy()).await.unwrap();
        let v = out
            .streams
            .iter()
            .find(|s| s.codec_type == "video")
            .unwrap();
        assert_eq!((v.width, v.height), (Some(1920), Some(1080)));
        assert!(t.segment(a, 0).await.is_ok());

        let other = root.join("other.mkv");
        std::fs::copy(&src, &other).unwrap();
        t.open(&other.to_string_lossy(), &plain).await.unwrap();
        assert!(t.playlist_path(a).await.is_none() && t.playlist_path(b).await.is_none());
        assert!(!root.join(a.to_string()).exists() && !root.join(b.to_string()).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn webview_codecs() {
        let p = |v: &str, a: Option<&str>| {
            let mut streams = vec![video(v, "High", "yuv420p", 41)];
            if let Some(a) = a {
                streams.push(audio(1, a, 2, 0));
            }
            probe(MP4, streams)
        };
        assert!(webview_plays(&p("h264", Some("aac"))));
        assert!(webview_plays(&p("hevc", None)));
        assert!(!webview_plays(&p("h264", Some("ac3"))));
        assert!(!webview_plays(&p("h264", Some("dts"))));
        assert!(!webview_plays(&p("mpeg4", Some("aac"))));
        assert!(!webview_plays(&Probe::default()));
    }

    #[test]
    fn chapters_sorted_titled_and_valid() {
        let json = r#"{"chapters":[
            {"start_time":"60.0","end_time":"120.0","tags":{"TITLE":"Middle"}},
            {"start_time":"0.0","end_time":"60.0","tags":{"title":"Intro"}},
            {"start_time":"120.0","end_time":"180.0"},
            {"start_time":"200.0","end_time":"200.0"},
            {"start_time":"x","end_time":"300.0"}]}"#;
        let probe: Probe = serde_json::from_str(json).unwrap();
        let c = chapters(&probe);
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].start, c[0].title.as_str()), (0.0, "Intro"));
        assert_eq!((c[1].start, c[1].title.as_str()), (60.0, "Middle"));
        assert_eq!((c[2].end, c[2].title.as_str()), (180.0, "Chapter 3"));
        assert!(chapters(&Probe::default()).is_empty());
    }

    #[tokio::test]
    async fn thumbnail_is_a_jpeg() {
        let root = std::env::temp_dir().join(format!("tonic-thumb-{}", std::process::id()));
        let Ok(t) = Transcoder::new(root.clone()) else {
            return;
        };
        let src = root.join("in.mp4");
        let made = std::process::Command::new(&t.ffmpeg)
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
        let jpeg = t
            .thumbnail(&src.to_string_lossy(), 2.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "JPEG start marker");
        assert!(
            t.thumbnail(&root.join("missing.mp4").to_string_lossy(), 1.0)
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_playlist_and_numbered_segments_are_served() {
        assert_eq!(parse_hls_name("index.m3u8"), Some(HlsFile::Playlist));
        assert_eq!(parse_hls_name("seg00042.ts"), Some(HlsFile::Segment(42)));
        for bad in [
            "seg.ts",
            "seg1.ts",
            "seg00001.ts.tmp",
            "../index.m3u8",
            "seg../1.ts",
            "x.ts",
            "ffmpeg.m3u8",
            "ffmpeg.log",
            "seg+0001.ts",
        ] {
            assert_eq!(parse_hls_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn restart_only_when_ffmpeg_cannot_reach_the_segment_soon() {
        assert!(needs_restart(None, &[], 0));
        // Running from 10, has written up to 12.
        assert!(!needs_restart(Some(10), &[10, 11, 12], 11));
        assert!(!needs_restart(Some(10), &[10, 11, 12], 14));
        assert!(needs_restart(Some(10), &[10, 11, 12], 15));
        assert!(needs_restart(Some(10), &[10, 11, 12], 9));
        // Nothing written yet in this run; an older segment does not count.
        assert!(!needs_restart(Some(10), &[], 12));
        assert!(!needs_restart(Some(10), &[3], 10));
        assert!(needs_restart(Some(10), &[3], 13));
        // Older segments next to this run's: only this run's tell how far it got.
        assert!(!needs_restart(Some(10), &[3, 10, 11, 12], 13));
    }

    #[test]
    fn window_keeps_a_minute_each_side() {
        let present: Vec<u32> = (0..=100).collect();
        let gone = outside_window(&present, 50);
        assert!(gone.contains(&34) && !gone.contains(&35));
        assert!(!gone.contains(&67) && gone.contains(&68));
        assert_eq!(outside_window(&[0, 1, 2], 0), Vec::<u32>::new());
    }

    fn job_with_segments(dir: &Path, segments: impl IntoIterator<Item = u32>) -> Job {
        std::fs::create_dir_all(dir).unwrap();
        for i in segments {
            std::fs::write(dir.join(segment_name(i)), b"ts").unwrap();
        }
        std::fs::write(dir.join("index.m3u8"), b"#EXTM3U").unwrap();
        Job {
            id: 1,
            input: "in".into(),
            plan: plan_of(None),
            dir: dir.into(),
            ffmpeg: None,
            last_request: 0,
        }
    }

    #[tokio::test]
    async fn tidy_keeps_the_window_and_stops_a_runaway_ffmpeg() {
        let dir = std::env::temp_dir().join(format!("tonic-tidy-{}", std::process::id()));
        let mut job = job_with_segments(&dir, 0..=60);
        let mut present = job.present();
        present.sort();
        assert_eq!(
            present,
            (0..=60).collect::<Vec<_>>(),
            "only segments are listed"
        );

        // No ffmpeg: nothing to stop, but segments far from playback go.
        job.last_request = 30;
        assert!(!job.tidy());
        let mut left = job.present();
        left.sort();
        assert_eq!(left, (15..=47).collect::<Vec<_>>());
        assert!(dir.join("index.m3u8").exists());

        // A stand-in for ffmpeg that has written up to segment 47.
        let sleeper = || {
            Command::new("sleep")
                .arg("30")
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        };
        job.ffmpeg = Some(Running {
            child: sleeper(),
            start: 20,
        });
        job.last_request = 33;
        assert!(job.tidy(), "47 is within a minute of 33: keep running");
        job.last_request = 32;
        assert!(
            job.tidy(),
            "47 is exactly a minute ahead of 32: keep running"
        );
        job.last_request = 31;
        assert!(!job.tidy(), "47 is past a minute ahead of 31: stop");
        assert!(job.ffmpeg.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn broken_input_fails_fast_with_ffmpeg_message() {
        let root = std::env::temp_dir().join(format!("tonic-broken-{}", std::process::id()));
        let Ok(t) = Transcoder::new(root.clone()) else {
            return;
        };
        let missing = root.join("missing.mkv").to_string_lossy().to_string();
        let id = t.open(&missing, &plan_of(None)).await.unwrap();
        let started = Instant::now();
        let error = t.segment(id, 0).await.unwrap_err().to_string();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gave up after {:?}",
            started.elapsed()
        );
        assert!(error.contains("ffmpeg"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The background tidier stops an ffmpeg that runs a minute ahead of the
    /// last request and keeps no segment beyond that window.
    #[tokio::test]
    async fn background_tidy_stops_ffmpeg_ahead_of_playback() {
        let root = std::env::temp_dir().join(format!("tonic-watch-{}", std::process::id()));
        let Ok(t) = Transcoder::new(root.clone()) else {
            return;
        };
        let src = root.join("long.mkv");
        let made = std::process::Command::new(&t.ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10:duration=200",
            ])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let input = src.to_string_lossy().to_string();
        let p = plan(&t.probe(&input).await.unwrap()).unwrap();
        let id = t.open(&input, &p).await.unwrap();
        t.segment(id, 0).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let stopped = t.jobs.lock().await.get(id).unwrap().ffmpeg.is_none();
            if stopped {
                break;
            }
            assert!(Instant::now() < deadline, "ffmpeg still running");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let present = t.jobs.lock().await.get(id).unwrap().present();
        assert!(present.iter().all(|&i| i <= AHEAD + 2), "{present:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_tools_on_path_and_not_missing_ones() {
        assert!(find_tool("sh", None).is_some_and(|p| p.is_file()));
        assert_eq!(find_tool("surely-not-a-tool-tonic", None), None);
    }

    #[test]
    fn reads_version_and_licence_from_ffmpeg_version_output() {
        let out = |config: &str| {
            format!(
                "ffmpeg version 9.0.2 Copyright (c) the FFmpeg developers\nconfiguration: {config}\n"
            )
        };
        let cases = [
            ("--enable-gpl --enable-version3", "GPL-3.0-or-later"),
            ("--enable-gpl", "GPL-2.0-or-later"),
            ("--enable-version3", "LGPL-3.0-or-later"),
            ("--disable-debug", "LGPL-2.1-or-later"),
        ];
        for (config, licence) in cases {
            assert_eq!(
                ffmpeg_about(&out(config)),
                ("9.0.2".into(), licence),
                "{config}"
            );
        }
        assert_eq!(ffmpeg_about("").0, "");
        let tagged = "ffmpeg version 9.0.2-https://example.org Copyright\nconfiguration: --extra-version='https://example.org' --enable-gpl\n";
        assert_eq!(ffmpeg_about(tagged).0, "9.0.2");
    }

    #[test]
    fn a_tool_beside_the_app_comes_before_path() {
        let dir = std::env::temp_dir().join(format!("tonic-beside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join(format!("sh{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&fake, "").unwrap();
        assert_eq!(find_tool("sh", Some(&dir)), Some(fake));
        assert!(find_tool("sh", Some(&dir.join("missing"))).is_some_and(|p| p.is_file()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End to end with the real ffmpeg: a 10-bit H.264 + 5.1 AC3 MKV comes out
    /// as 8-bit H.264 + stereo AAC, and a segment made by a restarted ffmpeg
    /// sits at its place on the timeline.
    #[tokio::test]
    async fn segments_on_demand_line_up() {
        let root = std::env::temp_dir().join(format!("tonic-hls-{}", std::process::id()));
        let Ok(t) = Transcoder::new(root.clone()) else {
            eprintln!("ffmpeg missing, skipped");
            return;
        };
        let src = root.join("in.mkv");
        let made = std::process::Command::new(&t.ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25:duration=14",
            ])
            .args(["-f", "lavfi", "-i", "sine=f=440:duration=14"])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p10le",
                "-c:a",
                "ac3",
                "-ac",
                "6",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let input = src.to_string_lossy().to_string();
        let p = plan(&t.probe(&input).await.unwrap()).unwrap();

        let id = t.open(&input, &p).await.unwrap();
        assert_eq!(
            t.open(&input, &p).await.unwrap(),
            id,
            "same input reuses the job"
        );
        let list = std::fs::read_to_string(t.playlist_path(id).await.unwrap()).unwrap();
        assert!(list.contains("seg00003.ts") && !list.contains("seg00004.ts"));
        assert!(t.segment(id, 4).await.is_err(), "past the playlist");

        let run_start = || async {
            t.jobs
                .lock()
                .await
                .get(id)
                .unwrap()
                .ffmpeg
                .as_ref()
                .map(|r| r.start)
        };
        // Too far ahead of the run that opened the job: restart there.
        t.segment(id, 3).await.unwrap();
        assert_eq!(run_start().await, Some(3));
        // Behind the current run: restart there.
        let later = t.segment(id, 2).await.unwrap();
        assert_eq!(run_start().await, Some(2));
        let first = t.segment(id, 0).await.unwrap();
        assert_eq!(run_start().await, Some(0));
        let start = |probe: &Probe| {
            probe
                .format
                .start_time
                .as_deref()
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        let p0 = t.probe(&first.to_string_lossy()).await.unwrap();
        let p2 = t.probe(&later.to_string_lossy()).await.unwrap();
        let gap = start(&p2) - start(&p0);
        assert!(
            (gap - 2.0 * SEG).abs() < 0.1,
            "segment 2 starts {gap} s after segment 0"
        );

        let v = p2.streams.iter().find(|s| s.codec_type == "video").unwrap();
        let a = p2.streams.iter().find(|s| s.codec_type == "audio").unwrap();
        assert_eq!(v.codec_name.as_deref(), Some("h264"));
        assert_eq!(v.pix_fmt.as_deref(), Some("yuv420p"));
        assert_eq!(a.codec_name.as_deref(), Some("aac"));
        assert_eq!(a.channels, Some(2));

        let other = root.join("other.mkv");
        std::fs::copy(&src, &other).unwrap();
        let id2 = t.open(&other.to_string_lossy(), &p).await.unwrap();
        assert_eq!(id2, id + 1);
        assert!(!root.join(id.to_string()).exists(), "old job dir removed");
        assert!(t.segment(id, 0).await.is_err(), "old job no longer served");
        let _ = std::fs::remove_dir_all(&root);
    }
}
