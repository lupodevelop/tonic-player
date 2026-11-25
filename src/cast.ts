// Chromecast and AirPlay. While casting the window goes black and the bar
// drives the device; when the session ends, playback resumes here where the
// device stopped.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { render } from "./controls";
import { $, devices, player, row, say, spinner, toast } from "./dom";
import { applyVr, hideVr } from "./picture";
import { finish, setSource } from "./playback";
import { app, ui, type Device } from "./state";

type CastEvent = { session: number } & (
  | { kind: "status"; time: number; duration: number | null; state: "playing" | "paused" | "buffering" }
  | { kind: "volume"; level: number; muted: boolean }
  | { kind: "ended"; time: number; finished: boolean }
);

const castButton = $<HTMLButtonElement>("cast");

export async function startCasting(device: Device, start = player.currentTime) {
  const current = app.current;
  if (!current) return;
  // Cast mode starts now, so nothing plays here while the device connects.
  app.casting = device.name;
  app.castDevice = device;
  hideVr();
  app.castSession = null;
  player.pause();
  $("cast-device").textContent = device.name;
  player.hidden = true;
  $("casting").hidden = false;
  say(`Casting to ${device.name}…`, true);
  try {
    app.castSession = await invoke<number>("cast_to", { device, source: current.source, start, vr: app.vrLayout });
    toast.hidden = true;
    ui.paused = false;
    render();
  } catch (err) {
    endCasting(start, true);
    say(String(err));
  }
}

function endCasting(time: number, finished: boolean) {
  app.casting = null;
  app.castSession = null;
  app.castDevice = null;
  spinner.hidden = true;
  $("casting").hidden = true;
  player.hidden = false;
  applyVr();
  player.currentTime = time;
  if (finished) finish();
  else player.play().catch(() => {});
}

/** Stops the device, waits until it closed the app, then plays on here. */
async function stopCasting() {
  const at = ui.time;
  await invoke("cast_control", { action: "stop" }).catch(() => {});
  if (app.casting) endCasting(at, false);
}

$("cast-stop").addEventListener("click", stopCasting);

listen<CastEvent>("cast", ({ payload }) => {
  if (!app.casting || payload.session !== app.castSession) return;
  if (payload.kind === "ended") return endCasting(payload.time, payload.finished);
  if (payload.kind === "volume") {
    ui.volume = payload.level;
    ui.muted = payload.muted;
  } else {
    ui.time = payload.time;
    if (payload.duration) ui.duration = payload.duration;
    ui.paused = payload.state === "paused";
    spinner.hidden = payload.state !== "buffering";
  }
  render();
});

export function closeDevices() {
  devices.hidden = true;
  castButton.setAttribute("aria-expanded", "false");
}

function deviceRow(device: Device) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = device.name;
  button.addEventListener("click", () => {
    closeDevices();
    startCasting(device);
  });
  return row(button);
}

castButton.addEventListener("click", async (e) => {
  e.stopPropagation();
  if (!devices.hidden) return closeDevices();
  devices.replaceChildren(row("Searching…"));
  devices.hidden = false;
  castButton.setAttribute("aria-expanded", "true");
  try {
    const found = await invoke<Device[]>("discover_devices");
    devices.replaceChildren(...(found.length ? found.map(deviceRow) : [row("No devices found")]));
  } catch (err) {
    devices.replaceChildren(row(String(err)));
  }
});

// AirPlay: WebKit's own picker. The receiver fetches the stream itself, so it
// needs the LAN address; the window plays from loopback until AirPlay is asked for.
$("airplay").addEventListener("click", async () => {
  const current = app.current;
  if (current && !player.currentSrc.includes(current.host)) {
    const at = player.currentTime;
    const paused = player.paused;
    await setSource(`http://${current.host}:${current.port}${current.path}`);
    player.currentTime = at;
    if (!paused) player.play().catch(() => {});
  }
  (player as HTMLVideoElement & { webkitShowPlaybackTargetPicker(): void }).webkitShowPlaybackTargetPicker();
});
