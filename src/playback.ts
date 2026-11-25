// Opening a video and playing it in the window, through to its end.
import { invoke } from "@tauri-apps/api/core";
import type Hls from "hls.js";
import { closeDevices, startCasting } from "./cast";
import { render } from "./controls";
import { $, bar, drawRanges, empty, player, say, spinner, toast, vrMenu } from "./dom";
import { pickEpisode, showChooser } from "./episodes";
import { nextEpisode, rate } from "./format";
import { applyVr, hideVr } from "./picture";
import { drawTicks, resetPreview } from "./seekbar";
import { app, ui, type Opened } from "./state";
import { showChrome, toggleMini } from "./window";

/** Opens a dropped path, a pasted or browser-sent magnet link, or a file:// URL. */
export async function load(input: string) {
  if (input.startsWith("file://")) input = decodeURIComponent(new URL(input).pathname);
  return open(input.startsWith("magnet:") ? "Fetching torrent…" : "Opening…", () => invoke<Opened>("open", { input }));
}

/**
 * Plays what `fetch` opens, or shows the list to pick from when it is a
 * folder or torrent with several videos. While casting, the new file goes to
 * the same device; the old session may lose its stream meanwhile, and its
 * events are ignored until the new one starts.
 */
export async function open(message: string, fetch: () => Promise<Opened>) {
  const device = app.castDevice;
  const previous = app.castSession;
  app.castSession = null;
  say(message, true);
  try {
    const opened = await fetch();
    toast.hidden = true;
    if (!opened.path) {
      // The old file is still on the device while the list is open.
      app.castSession = previous;
      return showChooser(opened);
    }
    play(opened);
    if (device) startCasting(device, 0);
  } catch (e) {
    app.castSession = previous;
    say(String(e));
  }
}

function play(opened: Opened) {
  app.current = opened;
  $("replay").hidden = true;
  $("title").textContent = opened.name;
  // WebGL may only read frames of a CORS video; the stream server allows it.
  // Loopback needs no local network permission, so playback starts at once.
  player.crossOrigin = "anonymous";
  setSource(`http://127.0.0.1:${opened.port}${opened.path}`);
  app.vrLayout = opened.vr;
  app.vrFlat = false;
  applyVr();
  // Nothing of the previous file may linger on the bar.
  ui.time = 0;
  ui.duration = 0;
  resetPreview();
  render();
  drawTicks();
  player.hidden = app.casting !== null;
  empty.hidden = true;
  bar.hidden = false;
  $("episodes").hidden = opened.episodes.length < 2;
  $("airplay").hidden = !("webkitShowPlaybackTargetPicker" in player);
  $("stats").hidden = opened.torrent === null;
  $("stats").textContent = "";
  $("have").replaceChildren();
  player.play().catch(() => {});
}

// WebKit on macOS plays HLS itself; WebView2 and WebKitGTK get hls.js.
let hls: Hls | null = null;
export async function setSource(url: string) {
  hls?.destroy();
  hls = null;
  if (url.endsWith(".m3u8") && !player.canPlayType("application/vnd.apple.mpegurl")) {
    const { default: Hls } = await import("hls.js");
    if (Hls.isSupported()) {
      hls = new Hls();
      hls.loadSource(url);
      hls.attachMedia(player);
      return;
    }
  }
  player.src = url;
}

// At the end the next video of the folder or torrent plays; after the last
// one the window goes back to the start page, which offers to play it again.
export async function finish() {
  const current = app.current;
  if (!current) return;
  const next = nextEpisode(current.episodes, current.file_id);
  if (next && (current.folder || current.torrent !== null)) return pickEpisode(next, current);
  if (app.mini) await toggleMini();
  player.pause();
  player.hidden = true;
  hideVr();
  bar.hidden = true;
  closeDevices();
  vrMenu.hidden = true;
  $("stats").hidden = true;
  $("replay").title = $("title").textContent ?? "";
  $("replay").hidden = false;
  empty.hidden = false;
  showChrome();
}

player.addEventListener("ended", () => {
  if (!app.casting) finish();
});

$("replay").addEventListener("click", () => {
  const current = app.current;
  if (!current) return;
  $("replay").hidden = true;
  empty.hidden = true;
  player.hidden = false;
  bar.hidden = false;
  $("stats").hidden = current.torrent === null;
  applyVr();
  player.currentTime = 0;
  player.play().catch(() => {});
});

// Safety net: the video here never plays while casting.
player.addEventListener("play", () => {
  if (app.casting) player.pause();
});

// Spinner while playback waits for data.
player.addEventListener("waiting", () => (spinner.hidden = app.casting !== null));
for (const event of ["playing", "pause", "canplay", "emptied", "error"]) {
  player.addEventListener(event, () => (spinner.hidden = true));
}

// Torrent health in the title bar: speed, peers and how much of the file is here.
type Stats = {
  peers: number;
  seen: number;
  down: number;
  up: number;
  file_done: number;
  file_len: number;
  have: [number, number][];
};
setInterval(async () => {
  const current = app.current;
  if (current?.torrent == null || current.file_id == null) return;
  try {
    const s = await invoke<Stats>("torrent_stats", { torrent: current.torrent, fileId: current.file_id });
    const pct = s.file_len ? Math.floor((s.file_done / s.file_len) * 100) : 0;
    $("stats").textContent = `↓ ${rate(s.down)} · ${s.peers} peers · ${pct}%`;
    $("stats").title = `${s.peers} connected, ${s.seen} known · ↑ ${rate(s.up)}`;
    drawRanges($("have"), s.have);
  } catch {
    $("stats").textContent = "";
  }
}, 1000);
