// How the picture is drawn over the video element: the VR view and the
// upscaler, and the menu that picks the VR layout.
import { startCasting } from "./cast";
import { togglePlay } from "./controls";
import { $, bar, player, row, say, vrMenu } from "./dom";
import { sameLayout } from "./format";
import { app, ui, type Vr } from "./state";
import { Upscaler } from "./upscale";
import { VrView } from "./vr";
import { zoom } from "./window";

// VR: a 360 or 180 video is drawn on a canvas over the video, as a view the
// mouse moves, or flat (one eye) in 2D mode. The Cast device gets a fixed,
// centred view rendered by ffmpeg.
const vrCanvas = $<HTMLCanvasElement>("vr");
let vrView: VrView | null = null;
try {
  vrView = new VrView(vrCanvas, player);
  vrView.onTap = togglePlay;
  vrCanvas.addEventListener("dblclick", zoom);
} catch (e) {
  console.warn("VR view unavailable:", e);
}

// Upscaling: FSR over the video whenever the window shows it larger than it
// is; a VR frame is scaled up 2x before it is projected. U turns both off and on.
let upscaler: Upscaler | null = null;
try {
  upscaler = new Upscaler($<HTMLCanvasElement>("upscaled"), player);
} catch (e) {
  console.warn("Upscaling unavailable:", e);
}

let upscaling = true;
export function toggleUpscale() {
  upscaling = !upscaling;
  if (upscaler) upscaler.enabled = upscaling;
  vrView?.setUpscale(upscaling);
  say(upscaling ? "Upscaling on" : "Upscaling off");
}

/** Shows the picture the way the state says: VR or not, and nothing while casting. */
export function applyVr() {
  if (upscaler) upscaler.blocked = app.casting !== null || app.vrLayout !== null;
  vrView?.show(app.casting ? null : app.vrLayout);
  vrView?.setFlat(app.vrFlat);
  bar.classList.toggle("vr-on", app.vrLayout !== null);
}

export function hideVr() {
  vrView?.show(null);
}

/** V: look around or one eye flat, for a VR video. */
export function toggleVrFlat() {
  if (!app.vrLayout) return;
  app.vrFlat = !app.vrFlat;
  applyVr();
}

const LAYOUTS: [string, Vr | null][] = [
  ["Normal video", null],
  ["360°", { fov: 360, stereo: "mono" }],
  ["360° top-bottom", { fov: 360, stereo: "tb" }],
  ["360° side by side", { fov: 360, stereo: "sbs" }],
  ["180°", { fov: 180, stereo: "mono" }],
  ["180° side by side", { fov: 180, stereo: "sbs" }],
  ["180° top-bottom", { fov: 180, stereo: "tb" }],
];

function menuItem(label: string, checked: boolean, act: () => void) {
  const button = document.createElement("button");
  button.type = "button";
  button.setAttribute("role", "menuitemcheckbox");
  button.setAttribute("aria-checked", String(checked));
  button.textContent = label;
  button.addEventListener("click", () => {
    vrMenu.hidden = true;
    act();
  });
  return row(button);
}

const setFlat = (flat: boolean) => () => {
  app.vrFlat = flat;
  applyVr();
};

$("vr-button").addEventListener("click", (e) => {
  e.stopPropagation();
  if (!vrMenu.hidden) return (vrMenu.hidden = true);
  const items = [];
  if (app.vrLayout) {
    items.push(
      menuItem("Look around", !app.vrFlat, setFlat(false)),
      menuItem("2D (one eye, flat)", app.vrFlat, setFlat(true)),
      document.createElement("hr"),
    );
  }
  for (const [label, layout] of LAYOUTS) {
    items.push(
      menuItem(label, sameLayout(layout, app.vrLayout), () => {
        app.vrLayout = layout;
        app.vrFlat = false;
        applyVr();
        // A running cast gets the new layout in the same session.
        if (app.castDevice) startCasting(app.castDevice, ui.time);
      }),
    );
  }
  vrMenu.replaceChildren(...items);
  vrMenu.hidden = false;
});
