// Entry point: what comes into the app (files, links, keys) and the updater.
// The other modules wire their own controls when they load.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import "./about";
import { closeDevices } from "./cast";
import { SKIP, VOLUME_STEP, seekTo, setVolume, toggleMute, togglePlay } from "./controls";
import { $, bar, chooser, devices, dialogOpen, empty, say, toast, vrMenu } from "./dom";
import { hideChooser } from "./episodes";
import { toggleUpscale, toggleVrFlat } from "./picture";
import { load } from "./playback";
import "./shield";
import { app, ui } from "./state";
import { toggleFullscreen, toggleMini, wake } from "./window";

// Keyboard: space/K play, arrows/J/L seek and volume, M mute, F full screen, P mini player, U upscaling, V VR/2D.
const KEYS: Record<string, () => void> = {
  Space: togglePlay,
  KeyK: togglePlay,
  ArrowLeft: () => seekTo(ui.time - SKIP),
  ArrowRight: () => seekTo(ui.time + SKIP),
  KeyJ: () => seekTo(ui.time - SKIP),
  KeyL: () => seekTo(ui.time + SKIP),
  ArrowUp: () => setVolume((ui.muted ? 0 : ui.volume) + VOLUME_STEP),
  ArrowDown: () => setVolume((ui.muted ? 0 : ui.volume) - VOLUME_STEP),
  KeyM: toggleMute,
  KeyF: toggleFullscreen,
  KeyP: toggleMini,
  KeyU: toggleUpscale,
  KeyV: toggleVrFlat,
};

document.addEventListener("keydown", (e) => {
  // Dialogs handle Esc and Tab themselves.
  if (dialogOpen()) return;
  if (!chooser.hidden) {
    if (e.code === "Escape" && app.current) hideChooser();
    return;
  }
  if (bar.hidden || e.metaKey || e.ctrlKey || e.altKey) return;
  const action = KEYS[e.code];
  if (!action) return;
  e.preventDefault();
  action();
  wake();
});

// Drag and drop: Tauri hands over real file paths.
getCurrentWebview().onDragDropEvent(({ payload }) => {
  document.body.classList.toggle("dragging", payload.type === "enter" || payload.type === "over");
  if (payload.type === "drop" && payload.paths[0]) load(payload.paths[0]);
});

empty.addEventListener("click", async () => {
  const path = await pickFile({
    filters: [{ name: "Video or torrent", extensions: ["mp4", "m4v", "webm", "mov", "mkv", "avi", "ts", "m2ts", "wmv", "flv", "mpg", "mpeg", "ogv", "torrent"] }],
  });
  if (path) load(path);
});

document.addEventListener("paste", (e) => {
  if (dialogOpen()) return;
  const text = e.clipboardData?.getData("text").trim();
  if (text) load(text);
});

// Files the system hands over: Open With, a double click, a second launch.
const takeIncoming = () => invoke<string[]>("take_incoming").then((files) => void (files[0] && load(files[0])));
listen("incoming", takeIncoming);
takeIncoming();

// Magnet links the browser hands to the app.
const loadFirst = (urls: string[] | null) => void (urls?.[0] && load(urls[0]));
onOpenUrl(loadFirst);
getCurrent().then(loadFirst);

// A narrow window folds controls behind the more button; it unfolds them on a second row.
$("more").addEventListener("click", () => {
  const open = bar.classList.toggle("more");
  $("more").setAttribute("aria-expanded", String(open));
});

// A click elsewhere closes the open popover.
document.addEventListener("click", (e) => {
  const target = e.target as Node;
  if (!devices.hidden && !devices.contains(target)) closeDevices();
  if (!vrMenu.hidden && !vrMenu.contains(target)) vrMenu.hidden = true;
});

// Updates come from the GitHub releases; the user decides when to install.
check()
  .then((update) => {
    if (!update) return;
    say(`Tonic Player ${update.version} is available. Click to update.`, true);
    toast.classList.add("action");
    toast.addEventListener("click", async function install() {
      // Only the update notice installs; later messages reuse the toast.
      if (!toast.classList.contains("action")) return;
      toast.removeEventListener("click", install);
      say("Updating…", true);
      try {
        await update.downloadAndInstall();
        await relaunch();
      } catch (e) {
        say(String(e));
      }
    });
  })
  .catch(() => {});
