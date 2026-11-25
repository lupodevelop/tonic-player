// The window: full screen, the mini player, the shape of vertical videos and
// the chrome that hides while the pointer rests.
import { invoke } from "@tauri-apps/api/core";
import { LogicalSize, PhysicalPosition, PhysicalSize, currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { $, bar, chooser, devices, dialogOpen, player, vrMenu } from "./dom";
import { miniBox, verticalWidth } from "./format";
import { app } from "./state";

type Frame = { position: PhysicalPosition; size: PhysicalSize };

export async function toggleFullscreen() {
  if (app.mini) await toggleMini();
  const win = getCurrentWindow();
  await win.setFullscreen(!(await win.isFullscreen()));
}

// Mini player: the window shrinks to a corner without window buttons,
// stays above other apps and on every desktop, and keeps its size handles.
// The previous frame is restored on the way back.
let beforeMini: Frame | null = null;
export async function toggleMini() {
  const win = getCurrentWindow();
  if (beforeMini) {
    const { position, size } = beforeMini;
    beforeMini = null;
    app.mini = false;
    document.body.classList.remove("mini");
    showChrome();
    await win.setAlwaysOnTop(false);
    await win.setVisibleOnAllWorkspaces(false);
    await win.setSize(size);
    await win.setPosition(position);
    return;
  }
  if (await win.isFullscreen()) await win.setFullscreen(false);
  beforeMini = { position: await win.outerPosition(), size: await win.innerSize() };
  app.mini = true;
  document.body.classList.add("mini");
  // No window buttons in the mini player: just the picture.
  invoke("window_buttons", { visible: false }).catch(() => {});
  // The longer side is 480 points, so a vertical video gets a tall, narrow box.
  const box = miniBox(player.videoWidth, player.videoHeight);
  await win.setSize(new LogicalSize(box.width, box.height));
  const monitor = await currentMonitor();
  if (monitor) {
    const scale = await win.scaleFactor();
    const margin = 24 * scale;
    const { width: w, height: h } = await win.outerSize();
    const x = monitor.position.x + monitor.size.width - w - margin;
    const y = monitor.position.y + monitor.size.height - h - margin * 3;
    await win.setPosition(new PhysicalPosition(Math.round(x), Math.round(y)));
  }
  await win.setAlwaysOnTop(true);
  await win.setVisibleOnAllWorkspaces(true);
}

/** Double click: the mini player grows back, otherwise full screen toggles. */
export function zoom() {
  return app.mini ? toggleMini() : toggleFullscreen();
}

// A vertical video narrows the window to its shape, keeping the height and
// the centre; the next horizontal video gets the earlier frame back. VR
// frames are skipped: a top-bottom 360° video is taller than wide.
let wide: Frame | null = null;
player.addEventListener("loadedmetadata", async () => {
  const win = getCurrentWindow();
  if (app.mini || app.vrLayout || !player.videoWidth || (await win.isFullscreen()) || (await win.isMaximized())) return;
  if (player.videoHeight > player.videoWidth) {
    const position = await win.outerPosition();
    const size = await win.innerSize();
    wide ??= { position, size };
    const width = verticalWidth(size.height, player.videoWidth, player.videoHeight);
    await win.setSize(new PhysicalSize(width, size.height));
    await win.setPosition(new PhysicalPosition(Math.round(position.x + (size.width - width) / 2), position.y));
  } else if (wide) {
    const { position, size } = wide;
    wide = null;
    await win.setSize(size);
    await win.setPosition(position);
  }
});

player.addEventListener("dblclick", zoom);
$("fullscreen").addEventListener("click", toggleFullscreen);
$("mini").addEventListener("click", toggleMini);
$("mini-expand").addEventListener("click", toggleMini);
$("mini-close").addEventListener("click", () => getCurrentWindow().close());

// Bar, title and window buttons show only while the pointer moves; they hide
// after 2.5 s of rest or when it leaves the window. They stay while the
// pointer is on the bar, a menu is open, or nothing is loaded.
let idleTimer = 0;
let idle = false;

function setIdle(value: boolean) {
  if (value === idle) return;
  idle = value;
  document.body.classList.toggle("idle", value);
  invoke("window_buttons", { visible: !value && !app.mini }).catch(() => {});
}

const mayHide = () =>
  !bar.hidden && !app.seeking && !bar.matches(":hover") && devices.hidden && vrMenu.hidden && chooser.hidden && !dialogOpen();

export function showChrome() {
  setIdle(false);
  invoke("window_buttons", { visible: !app.mini }).catch(() => {});
}

export function wake() {
  setIdle(false);
  clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => (mayHide() ? setIdle(true) : wake()), 2500);
}
document.addEventListener("mousemove", wake);
document.documentElement.addEventListener("mouseleave", () => {
  clearTimeout(idleTimer);
  if (mayHide()) setIdle(true);
});
