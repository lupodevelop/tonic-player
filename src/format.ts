// Pure helpers: formatting and the geometry the window and the bar need.
import type { Vr } from "./state";

export function clock(s: number) {
  const t = Math.max(0, Math.floor(s || 0));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = String(t % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

export function size(bytes: number) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`;
}

export function rate(bps: number) {
  return bps >= 1e6 ? `${(bps / 1e6).toFixed(1)} MB/s` : `${Math.round(bps / 1e3)} KB/s`;
}

export function sameLayout(a: Vr | null, b: Vr | null) {
  return a?.fov === b?.fov && a?.stereo === b?.stereo;
}

/** Where `x` falls across a box, from 0 to 1. */
export function fraction(x: number, left: number, width: number) {
  return Math.min(1, Math.max(0, (x - left) / width)) || 0;
}

/** Left of the preview card, relative to the bar, kept `half` px inside the window. */
export function previewLeft(x: number, barLeft: number, windowWidth: number, half = 96) {
  return Math.min(windowWidth - half - barLeft, Math.max(half - barLeft, x - barLeft));
}

/** Mini player box in points: the longer side is 480, in the video's shape (16:9 when unknown). */
export function miniBox(videoWidth: number, videoHeight: number) {
  const aspect = videoWidth && videoHeight ? videoHeight / videoWidth : 9 / 16;
  const width = aspect > 1 ? 480 / aspect : 480;
  return { width: Math.round(width), height: Math.round(width * aspect) };
}

/** Width that gives a window of `height` the shape of the video. */
export function verticalWidth(height: number, videoWidth: number, videoHeight: number) {
  return Math.round((height * videoWidth) / videoHeight);
}

export function nextEpisode<T extends { file_id: number }>(list: T[], playing: number | null) {
  const at = list.findIndex((e) => e.file_id === playing);
  return at >= 0 ? list[at + 1] : undefined;
}

/** Preview frame time: the nearest even second, close to the pointer, so repeat visits hit the cache. */
export function thumbTime(t: number, duration: number) {
  return Math.min(Math.round(t / 2) * 2, Math.floor(duration));
}
