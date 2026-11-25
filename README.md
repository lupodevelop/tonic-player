<p align="center">
  <img src="assets/app-icon.svg" width="128" alt="" />
</p>

<h1 align="center">Tonic Player</h1>

<p align="center">Video player for local files and torrents, streamed while they download, with Chromecast and AirPlay.</p>

<p align="center">
  <img src="assets/screenshot.png" width="720" alt="Tonic Player start window" />
</p>

## Features

- Drop a video, a folder or a `.torrent` file on the window; magnet links open from the browser. A folder or a torrent with several videos shows a list to pick from.
- Torrents play while they download.
- Formats the window cannot play (MKV, AVI and others) are converted on the fly, with free seeking.
- Chromecast: the control bar drives the device; when the cast ends, playback goes on in the window. AirPlay on macOS.
- 360° and 180° VR videos: drag to look around, or watch one eye flat. A Chromecast gets a centred view.
- Seek bar previews, chapters, torrent speed and peers.
- Videos below HD shown larger than they are get scaled up with AMD FSR 1 on the GPU.
- Mini player: a small, resizable window that stays above other apps.
- Shield: torrents go through the SOCKS5 proxy of a VPN provider, so peers do not see your address.
- Nothing is kept on disk but the shield setting: torrent data and converted segments are deleted when the app quits.

Keyboard: space or K play/pause, ← → or J L seek 10 s, ↑ ↓ volume, M mute, F full screen, P mini player, U upscaling, V VR/2D.

## Install

Download the build for your system from [Releases](https://github.com/lupodevelop/tonic-player/releases). Installed apps check for updates at start.

The builds are not signed. On macOS, open the app the first time with right click → Open. On Windows, choose More info → Run anyway.

The app bundles [FFmpeg](https://ffmpeg.org) (GPL build); nothing else needs to be installed.

macOS is the tested platform. The Windows and Linux builds are not tested yet.

## Build

Requirements: Rust (stable), Node.js with pnpm, [cargo-about](https://github.com/EmbarkStudios/cargo-about) (for `pnpm tauri build`; `cargo install cargo-about --locked --features cli`), FFmpeg on PATH (for the tests), and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm fetch:ffmpeg   # download the pinned FFmpeg build for this system
pnpm tauri dev      # run
pnpm tauri build    # package for this system
pnpm test           # frontend tests
cd src-tauri && cargo test
```

`pnpm fetch:ffmpeg` writes ffmpeg and ffprobe to `src-tauri/binaries/`, checked against the sha256 in `scripts/ffmpeg.json`, or against the release's `SHA256SUMS` when none is pinned; `pnpm tauri dev` and `pnpm tauri build` run it when the files are missing. `pnpm notices` writes the third-party licences shown in the About popup.

The bundled FFmpeg is a minimal build with only what the app uses. `scripts/build-ffmpeg.sh <target-triple>` makes it; the FFmpeg workflow, run by hand for each FFmpeg version, builds every target and publishes the binaries and their source tarballs as the release `ffmpeg-<version>`.

`cargo test -- --ignored` also runs a test that downloads a public torrent.

## Release

Set the version in `src-tauri/tauri.conf.json`, then push a tag `v<version>`. The release workflow builds macOS (Apple Silicon and Intel), Windows and Linux, signs the update files and creates a draft release; publishing it makes the update available.

The workflow needs the secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; the public key is in `tauri.conf.json`.

## Limits

- The shield needs a SOCKS5 proxy from a VPN provider; the app does not offer one. With it on, DHT and UDP trackers are off, so torrents find fewer peers and a magnet link needs an HTTP tracker. The proxy address is saved unencrypted.
- Converted video is re-encoded to H.264/AAC about a minute ahead of playback; HDR is not tone mapped.
- The stream server listens on every interface so that cast devices can reach it. Its paths carry a random token per run, but anyone who sees a stream URL on the network can read that file.

## License

[GPL-3.0-or-later](LICENSE)
