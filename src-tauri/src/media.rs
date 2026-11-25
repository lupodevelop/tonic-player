use std::path::Path;

use serde::{Deserialize, Serialize};

/// Parses a single-range `Range` header against a resource of `len` bytes.
/// `Ok(None)` means no usable range was asked for and the whole body is sent;
/// `Err(())` means the range cannot be satisfied (HTTP 416).
/// Multi-range requests are answered with the whole body, which RFC 9110 allows.
pub fn parse_range(header: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    let (start, end) = (start.trim(), end.trim());
    if len == 0 {
        return Err(());
    }
    let range = if start.is_empty() {
        let suffix: u64 = end.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (len.saturating_sub(suffix), len - 1)
    } else {
        let start: u64 = start.parse().map_err(|_| ())?;
        let end = if end.is_empty() {
            len - 1
        } else {
            end.parse::<u64>().map_err(|_| ())?.min(len - 1)
        };
        if start > end {
            return Err(());
        }
        (start, end)
    };
    Ok(Some(range))
}

/// MIME type of a video file, by extension. MP4, WebM and QuickTime play in
/// the webview as they are; the rest goes through the transcoder.
pub fn content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "avi" | "divx" => Some("video/x-msvideo"),
        "ts" | "m2ts" | "mts" => Some("video/mp2t"),
        "wmv" | "asf" => Some("video/x-ms-wmv"),
        "flv" => Some("video/x-flv"),
        "mpg" | "mpeg" | "vob" => Some("video/mpeg"),
        "ogv" => Some("video/ogg"),
        "3gp" => Some("video/3gpp"),
        _ => None,
    }
}

/// Whether a file's first bytes are those of a video container. A folder can
/// hold files with a video extension that are not videos: macOS's `._` side
/// files and still images in the MP4 family (HEIC, AVIF).
pub fn looks_like_video(head: &[u8]) -> bool {
    let at = |i: usize, sig: &[u8]| head.get(i..i + sig.len()) == Some(sig);
    if at(4, b"ftyp") {
        let images: [&[u8]; 9] = [
            b"heic", b"heix", b"heim", b"heis", b"hevc", b"mif1", b"msf1", b"avif", b"avis",
        ];
        return head
            .get(8..12)
            .is_some_and(|brand| !images.contains(&brand));
    }
    [b"moov", b"mdat", b"wide", b"free", b"skip", b"pnot"]
        .iter()
        .any(|atom| at(4, *atom))
        || at(0, &[0x1A, 0x45, 0xDF, 0xA3])
        || (at(0, b"RIFF") && at(8, b"AVI "))
        || at(0, &[0x47])
        || at(4, &[0x47])
        || at(0, &[0x30, 0x26, 0xB2, 0x75])
        || at(0, b"FLV")
        || at(0, &[0, 0, 1, 0xBA])
        || at(0, &[0, 0, 1, 0xB3])
        || at(0, b"OggS")
}

/// Indices of the files with a known video type, in natural name order, so
/// that "E2" comes before "E10".
pub fn video_files<'a>(files: impl IntoIterator<Item = &'a Path>) -> Vec<usize> {
    let mut videos: Vec<(usize, &Path)> = files
        .into_iter()
        .enumerate()
        .filter(|(_, path)| content_type(path).is_some())
        .collect();
    videos.sort_by(|a, b| natural_cmp(&a.1.to_string_lossy(), &b.1.to_string_lossy()));
    videos.into_iter().map(|(i, _)| i).collect()
}

/// The videos directly inside `dir` with their sizes, in natural order.
/// Hidden files are skipped, and so is any file whose first bytes are not
/// those of a video, whatever its extension.
pub fn folder_videos(dir: &Path) -> std::io::Result<Vec<(std::path::PathBuf, u64)>> {
    use std::io::Read;
    let sniff = |path: &Path| {
        let mut head = [0u8; 16];
        let read = std::fs::File::open(path).and_then(|mut f| f.read(&mut head));
        read.is_ok_and(|n| looks_like_video(&head[..n]))
    };
    let files: Vec<(std::path::PathBuf, u64)> = std::fs::read_dir(dir)?
        .filter_map(|e| {
            let entry = e.ok()?;
            let meta = entry.metadata().ok()?;
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            (meta.is_file() && !hidden && sniff(&path)).then_some((path, meta.len()))
        })
        .collect();
    let order = video_files(files.iter().map(|(p, _)| p.as_path()));
    Ok(order.into_iter().map(|i| files[i].clone()).collect())
}

/// Compares strings with runs of digits taken as numbers, ignoring case.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    let trimmed = digits.trim_start_matches('0').to_string();
                    (trimmed.len(), trimmed)
                };
                let order = number(&mut a).cmp(&number(&mut b));
                if order.is_ne() {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order.is_ne() {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Display titles for files of one torrent: the path inside the torrent
/// without the folders every file shares and without the extension, so a
/// season of `Ep 01/video.mkv`, `Ep 02/video.mkv` reads `Ep 01 › video`.
pub fn titles(paths: &[&Path]) -> Vec<String> {
    let parts: Vec<Vec<String>> = paths
        .iter()
        .map(|p| p.iter().map(|c| c.to_string_lossy().into_owned()).collect())
        .collect();
    // Shared leading folders; the file name itself is never dropped.
    let shortest = parts
        .iter()
        .map(|p| p.len().saturating_sub(1))
        .min()
        .unwrap_or(0);
    let shared = (0..shortest)
        .take_while(|&i| parts.iter().all(|p| p[i] == parts[0][i]))
        .count();
    let shared = if paths.len() > 1 {
        shared
    } else {
        parts.first().map_or(0, |p| p.len().saturating_sub(1))
    };
    parts
        .iter()
        .map(|p| {
            let mut rest: Vec<String> = p[shared..].to_vec();
            if let Some(last) = rest.last_mut()
                && let Some(stem) = Path::new(last.as_str()).file_stem()
            {
                *last = stem.to_string_lossy().into_owned();
            }
            rest.join(" › ")
        })
        .collect()
}

/// Runs of downloaded pieces inside `pieces` (a file's piece range), as
/// `(start, end)` fractions of that range for drawing on a seek bar.
pub fn have_ranges(pieces: std::ops::Range<u32>, have: impl Fn(u32) -> bool) -> Vec<(f64, f64)> {
    let total = pieces.len() as f64;
    let mut out = Vec::new();
    let mut run: Option<u32> = None;
    for p in pieces.clone().chain(std::iter::once(pieces.end)) {
        let got = p < pieces.end && have(p);
        match (got, run) {
            (true, None) => run = Some(p),
            (false, Some(start)) => {
                let frac = |x: u32| (x - pieces.start) as f64 / total;
                out.push((frac(start), frac(p)));
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// Layout of a VR video: how much of the sphere it covers and how the two
/// eyes are packed in the frame.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Vr {
    /// 360 or 180 degrees.
    pub fov: u16,
    pub stereo: Stereo,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Stereo {
    Mono,
    /// Side by side, left eye first.
    Sbs,
    /// Top and bottom, left eye on top.
    Tb,
}

/// Recognises a VR video from its spherical and stereo metadata (as ffprobe
/// reports them), else from the usual file name tags (`360`, `180`, `VR`,
/// `SBS`/`LR`, `TB`/`OU`). Without any sign of VR, a plain or 3D film is not
/// VR. When the coverage is not named, an eye close to square is taken as
/// 180 degrees and anything wider as 360.
pub fn detect_vr(
    name: &str,
    width: u32,
    height: u32,
    spherical: bool,
    stereo3d: Option<&str>,
) -> Option<Vr> {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    let fov_tag = if has(&["180", "vr180"]) {
        Some(180)
    } else if has(&["360", "vr360"]) {
        Some(360)
    } else {
        None
    };
    if !spherical && fov_tag.is_none() && !has(&["vr"]) {
        return None;
    }
    let stereo = match stereo3d.map(str::to_ascii_lowercase).as_deref() {
        Some(t) if t.contains("side") => Stereo::Sbs,
        Some(t) if t.contains("top") => Stereo::Tb,
        _ if has(&["sbs", "lr", "3dh", "sidebyside"]) => Stereo::Sbs,
        _ if has(&["tb", "ou", "3dv", "overunder", "topbottom"]) => Stereo::Tb,
        _ => Stereo::Mono,
    };
    let fov = fov_tag.unwrap_or_else(|| {
        let (w, h) = match stereo {
            Stereo::Sbs => (width / 2, height),
            Stereo::Tb => (width, height / 2),
            Stereo::Mono => (width, height),
        };
        if h > 0 && (w as f64 / h as f64) < 1.5 {
            180
        } else {
            360
        }
    });
    Some(Vr { fov, stereo })
}

/// Only magnet links reach the torrent engine: its URL input would also fetch
/// arbitrary http(s) URLs.
pub fn is_magnet(input: &str) -> bool {
    input
        .trim()
        .get(..20)
        .is_some_and(|p| p.eq_ignore_ascii_case("magnet:?xt=urn:btih:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: u64 = 1000;

    #[test]
    fn range_absent_or_foreign_unit_sends_whole_body() {
        assert_eq!(parse_range(None, LEN), Ok(None));
        assert_eq!(parse_range(Some("items=0-1"), LEN), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), LEN), Ok(None));
    }

    #[test]
    fn range_closed_open_and_suffix() {
        assert_eq!(parse_range(Some("bytes=10-19"), LEN), Ok(Some((10, 19))));
        assert_eq!(parse_range(Some("bytes=10-"), LEN), Ok(Some((10, LEN - 1))));
        assert_eq!(
            parse_range(Some("bytes=-100"), LEN),
            Ok(Some((LEN - 100, LEN - 1)))
        );
    }

    #[test]
    fn range_is_clamped_to_resource() {
        assert_eq!(
            parse_range(Some("bytes=10-99999"), LEN),
            Ok(Some((10, LEN - 1)))
        );
        assert_eq!(
            parse_range(Some("bytes=-99999"), LEN),
            Ok(Some((0, LEN - 1)))
        );
        assert_eq!(parse_range(Some("bytes=0-0"), LEN), Ok(Some((0, 0))));
        assert_eq!(parse_range(Some("bytes=999-"), LEN), Ok(Some((999, 999))));
    }

    #[test]
    fn range_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=1000-"), LEN), Err(()));
        assert_eq!(parse_range(Some("bytes=20-10"), LEN), Err(()));
        assert_eq!(parse_range(Some("bytes=-0"), LEN), Err(()));
        assert_eq!(parse_range(Some("bytes=0-"), 0), Err(()));
        assert_eq!(parse_range(Some("bytes=abc"), LEN), Err(()));
        assert_eq!(parse_range(Some("bytes=a-b"), LEN), Err(()));
    }

    #[test]
    fn content_type_by_extension_case_insensitive() {
        assert_eq!(content_type(Path::new("a/B.MP4")), Some("video/mp4"));
        assert_eq!(content_type(Path::new("x.m4v")), Some("video/mp4"));
        assert_eq!(content_type(Path::new("x.webm")), Some("video/webm"));
        assert_eq!(content_type(Path::new("x.mkv")), Some("video/x-matroska"));
        assert_eq!(
            content_type(Path::new("Season 1/x.AVI")),
            Some("video/x-msvideo")
        );
        assert_eq!(content_type(Path::new("x.mov")), Some("video/quicktime"));
        assert_eq!(content_type(Path::new("x.m2ts")), Some("video/mp2t"));
        assert_eq!(content_type(Path::new("x.srt")), None);
        assert_eq!(content_type(Path::new("noext")), None);
    }

    #[test]
    fn lists_videos_in_natural_order() {
        let files = [
            Path::new("Show/E10.mkv"),
            Path::new("Show/readme.txt"),
            Path::new("Show/e2.mkv"),
            Path::new("Show/E1.MP4"),
            Path::new("Show/E03.webm"),
        ];
        assert_eq!(video_files(files), vec![3, 2, 4, 0]);
        assert_eq!(video_files([Path::new("a.nfo")]), Vec::<usize>::new());
    }

    #[test]
    fn folder_lists_only_real_videos() {
        let dir = std::env::temp_dir().join(format!("tonic-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Extras.mp4")).unwrap();
        let mp4 = [&[0, 0, 0, 24][..], b"ftypisom", &[0; 12]].concat();
        let heic = [&[0, 0, 0, 24][..], b"ftypheic", &[0; 12]].concat();
        for (name, bytes) in [
            ("E10.mkv", vec![0x1A, 0x45, 0xDF, 0xA3, 1]),
            ("E2.mp4", mp4.clone()),
            ("._E2.mp4", vec![0, 5, 22, 7, 0, 2, 0, 0]),
            (".hidden.mp4", mp4.clone()),
            ("IMG_0001.mp4", heic),
            ("playlist.m3u", b"#EXTM3U\n".to_vec()),
            ("cover.jpg", vec![0xFF, 0xD8, 0xFF]),
            ("empty.mp4", vec![]),
        ] {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let found = folder_videos(&dir).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["E2.mp4", "E10.mkv"]);
        assert_eq!(found[0].1, mp4.len() as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recognises_video_containers_and_not_images() {
        let iso = |brand: &[u8]| [&[0, 0, 0, 24][..], b"ftyp", brand, &[0; 4]].concat();
        for video in [
            iso(b"isom"),
            iso(b"qt  "),
            iso(b"3gp4"),
            [&[0, 0, 0, 8][..], b"wide", &[0; 8]].concat(),
            vec![0x1A, 0x45, 0xDF, 0xA3, 1, 0, 0, 0],
            [&b"RIFF"[..], &[0; 4], b"AVI LIST"].concat(),
            vec![0x47, 0x40, 0, 0x10],
            vec![0, 0, 0, 0, 0x47, 0x40],
            vec![0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66],
            b"FLV\x01\x05".to_vec(),
            vec![0, 0, 1, 0xBA, 0x44],
            b"OggS\0\x02".to_vec(),
        ] {
            assert!(looks_like_video(&video), "{video:?}");
        }
        for other in [
            iso(b"heic"),
            iso(b"avif"),
            iso(b"mif1"),
            [&[0, 5, 22, 7, 0, 2, 0, 0][..], b"Mac OS X        "].concat(),
            [&b"RIFF"[..], &[0; 4], b"WEBPVP8 "].concat(),
            b"#EXTM3U\n".to_vec(),
            vec![0xFF, 0xD8, 0xFF, 0xE0],
            vec![],
            b"ftyp".to_vec(),
        ] {
            assert!(!looks_like_video(&other), "{other:?}");
        }
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("E2", "E10"), Less);
        assert_eq!(natural_cmp("E10", "E2"), Greater);
        assert_eq!(natural_cmp("e007", "E7"), Equal);
        assert_eq!(natural_cmp("S01E09", "S02E01"), Less);
        assert_eq!(natural_cmp("abc", "abd"), Less);
        assert_eq!(natural_cmp("ab", "abc"), Less);
        assert_eq!(natural_cmp("abc", "ab"), Greater);
        assert_eq!(natural_cmp("", ""), Equal);
    }

    #[test]
    fn titles_drop_shared_folders_and_extension() {
        let paths = [
            Path::new("Show S01/Ep 01 - Pilot/video(1).mkv"),
            Path::new("Show S01/Ep 02 - Second/video(2).mkv"),
        ];
        assert_eq!(
            titles(&paths),
            vec!["Ep 01 - Pilot › video(1)", "Ep 02 - Second › video(2)"]
        );
        let flat = [
            Path::new("Show/E01 Title.mkv"),
            Path::new("Show/E02 Other.mp4"),
        ];
        assert_eq!(titles(&flat), vec!["E01 Title", "E02 Other"]);
        let mixed = [Path::new("Show/a.mkv"), Path::new("Show/Extras/b.mkv")];
        assert_eq!(titles(&mixed), vec!["a", "Extras › b"]);
        assert_eq!(
            titles(&[Path::new("Movie/Movie.2020.mkv")]),
            vec!["Movie.2020"]
        );
        assert_eq!(titles(&[]), Vec::<String>::new());
    }

    #[test]
    fn have_ranges_are_runs_relative_to_the_file() {
        let have = |p: u32| matches!(p, 10 | 11 | 14 | 19);
        assert_eq!(
            have_ranges(10..20, have),
            vec![(0.0, 0.2), (0.4, 0.5), (0.9, 1.0)]
        );
        assert_eq!(have_ranges(10..20, |_| true), vec![(0.0, 1.0)]);
        assert_eq!(have_ranges(10..20, |_| false), vec![]);
        assert_eq!(have_ranges(5..5, |_| true), vec![]);
    }

    const fn vr(fov: u16, stereo: Stereo) -> Option<Vr> {
        Some(Vr { fov, stereo })
    }

    #[test]
    fn vr_from_file_name_tags() {
        assert_eq!(
            detect_vr("Trip_360.mp4", 3840, 1920, false, None),
            vr(360, Stereo::Mono)
        );
        assert_eq!(
            detect_vr("Concert 180 LR.mp4", 5760, 2880, false, None),
            vr(180, Stereo::Sbs)
        );
        assert_eq!(
            detect_vr("dive_360_TB.mkv", 3840, 3840, false, None),
            vr(360, Stereo::Tb)
        );
        assert_eq!(
            detect_vr("walk.VR180.SBS.mp4", 4096, 2048, false, None),
            vr(180, Stereo::Sbs)
        );
        assert_eq!(
            detect_vr("scene_OU_360.mp4", 4096, 4096, false, None),
            vr(360, Stereo::Tb)
        );
    }

    #[test]
    fn vr_from_metadata() {
        assert_eq!(
            detect_vr("clip.mp4", 3840, 1920, true, None),
            vr(360, Stereo::Mono)
        );
        assert_eq!(
            detect_vr("clip.mp4", 3840, 3840, true, Some("top and bottom")),
            vr(360, Stereo::Tb)
        );
        assert_eq!(
            detect_vr("clip.mp4", 4096, 2048, true, Some("side by side")),
            vr(180, Stereo::Sbs)
        );
    }

    #[test]
    fn vr_coverage_guessed_from_eye_shape() {
        // Tagged VR without 180/360: square eyes mean 180.
        assert_eq!(
            detect_vr("park_vr_sbs.mp4", 4096, 2048, false, None),
            vr(180, Stereo::Sbs)
        );
        assert_eq!(
            detect_vr("park_vr.mp4", 4096, 2048, false, None),
            vr(360, Stereo::Mono)
        );
        assert_eq!(
            detect_vr("park_vr.mp4", 2048, 2048, false, None),
            vr(180, Stereo::Mono)
        );
        assert_eq!(
            detect_vr("park_vr.mp4", 0, 0, false, None),
            vr(360, Stereo::Mono)
        );
    }

    #[test]
    fn plain_and_3d_films_are_not_vr() {
        assert_eq!(
            detect_vr("Movie.2020.1080p.mkv", 1920, 1080, false, None),
            None
        );
        assert_eq!(
            detect_vr("Movie.3D.SBS.mkv", 3840, 1080, false, Some("side by side")),
            None
        );
        assert_eq!(detect_vr("Tracking.360p.mp4", 640, 360, false, None), None);
        assert_eq!(detect_vr("Server.vrm.mp4", 1920, 1080, false, None), None);
    }

    #[test]
    fn only_btih_magnets_accepted() {
        assert!(is_magnet("magnet:?xt=urn:btih:abc"));
        assert!(is_magnet("  MAGNET:?xt=urn:btih:abc"));
        assert!(!is_magnet("https://example.com/a.torrent"));
        assert!(!is_magnet("magnet:?dn=x"));
        assert!(!is_magnet("/etc/passwd"));
        assert!(!is_magnet(""));
    }
}
