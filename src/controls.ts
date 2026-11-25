// The control bar: play, seek and volume, drawn from `ui`. Every control goes
// to the Cast device while casting, else to the video here.
import { invoke } from "@tauri-apps/api/core";
import { $, bar, player, say } from "./dom";
import { clock, fraction } from "./format";
import { app, ui } from "./state";

export const SKIP = 10;
export const VOLUME_STEP = 0.05;

const seek = $<HTMLInputElement>("seek");
const volume = $<HTMLDivElement>("volume");

export function render() {
  bar.classList.toggle("paused", ui.paused);
  bar.classList.toggle("muted", ui.muted || ui.volume === 0);
  $("toggle").setAttribute("aria-label", ui.paused ? "Play" : "Pause");
  $("mute").setAttribute("aria-label", ui.muted ? "Unmute" : "Mute");
  seek.max = String(ui.duration || 0);
  const shown = app.seeking ? Number(seek.value) : ui.time;
  const played = ui.duration ? `${(shown / ui.duration) * 100}%` : "0";
  $("played").style.width = played;
  $("knob").style.left = played;
  if (!app.seeking) seek.value = String(ui.time);
  $("time").textContent = clock(shown);
  $("duration").textContent = ui.duration ? clock(ui.duration) : "–:––";
  const level = ui.muted ? 0 : ui.volume;
  bar.classList.toggle("quiet", level < 0.5);
  $("volume-fill").style.width = `${level * 100}%`;
  // The knob travels the track, which is inset 8 px on each side.
  $("volume-knob").style.left = `calc(8px + (100% - 16px) * ${level})`;
  volume.setAttribute("aria-valuenow", String(Math.round(level * 100)));
}

export function control(action: string, time?: number) {
  return invoke("cast_control", { action, time }).catch((e) => say(String(e)));
}

export function togglePlay() {
  if (app.casting) {
    ui.paused = !ui.paused;
    control(ui.paused ? "pause" : "play");
    return render();
  }
  if (player.paused) player.play().catch(() => {});
  else player.pause();
}

export function seekTo(t: number) {
  const time = Math.min(Math.max(0, t), ui.duration || Infinity);
  ui.time = time;
  if (app.casting) control("seek", time);
  else player.currentTime = time;
  render();
}

// A drag on the slider would send a Cast command per pixel: the level goes
// out once the drag pauses for 150 ms.
let castVolumeTimer = 0;
export function setVolume(v: number) {
  const level = Math.min(1, Math.max(0, v));
  ui.volume = level;
  ui.muted = false;
  if (app.casting) {
    clearTimeout(castVolumeTimer);
    castVolumeTimer = window.setTimeout(() => control("volume", ui.volume), 150);
  } else {
    player.volume = level;
    player.muted = false;
  }
  render();
}

export function toggleMute() {
  ui.muted = !ui.muted;
  if (app.casting) control(ui.muted ? "mute" : "unmute");
  else player.muted = ui.muted;
  render();
}

// The video here feeds the bar unless a Cast session does.
for (const event of ["timeupdate", "durationchange", "play", "pause", "volumechange", "loadedmetadata"]) {
  player.addEventListener(event, () => {
    if (app.casting) return;
    ui.time = player.currentTime;
    ui.duration = Number.isFinite(player.duration) ? player.duration : 0;
    ui.paused = player.paused;
    ui.volume = player.volume;
    ui.muted = player.muted;
    render();
  });
}

player.addEventListener("click", togglePlay);
$("toggle").addEventListener("click", togglePlay);
$("back").addEventListener("click", () => seekTo(ui.time - SKIP));
$("forward").addEventListener("click", () => seekTo(ui.time + SKIP));
$("mute").addEventListener("click", toggleMute);

// Click and drag on the bar map the pointer over its full width, the same
// mapping the preview uses (the native slider insets by its thumb).
const scrub = $("scrub");
const scrubTime = (e: PointerEvent) => {
  const box = scrub.getBoundingClientRect();
  return fraction(e.clientX, box.left, box.width) * ui.duration;
};
scrub.addEventListener("pointerdown", (e) => {
  if (!ui.duration) return;
  scrub.setPointerCapture(e.pointerId);
  app.seeking = true;
  seek.value = String(scrubTime(e));
  render();
});
scrub.addEventListener("pointermove", (e) => {
  if (!app.seeking) return;
  seek.value = String(scrubTime(e));
  render();
});
scrub.addEventListener("pointerup", (e) => {
  if (!app.seeking) return;
  app.seeking = false;
  seekTo(scrubTime(e));
});

// Volume: click or drag anywhere on the slider; the wheel works over the
// speaker too.
const volumeAt = (e: PointerEvent) => {
  const track = volume.querySelector(".track")!.getBoundingClientRect();
  return fraction(e.clientX, track.left, track.width);
};
volume.addEventListener("pointerdown", (e) => {
  volume.setPointerCapture(e.pointerId);
  $("audio").classList.add("adjusting");
  setVolume(volumeAt(e));
});
volume.addEventListener("pointermove", (e) => {
  if (volume.hasPointerCapture(e.pointerId)) setVolume(volumeAt(e));
});
volume.addEventListener("pointerup", () => $("audio").classList.remove("adjusting"));
$("audio").addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    setVolume((ui.muted ? 0 : ui.volume) - e.deltaY * 0.002);
  },
  { passive: false },
);
