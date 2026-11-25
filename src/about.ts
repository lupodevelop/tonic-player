// About: a modal dialog (Esc, the close button or a click outside closes it,
// and focus goes back where it was). Its links open in the browser. The
// third-party notices are built with the app; a dev run has none.
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { closeDevices } from "./cast";
import { $, closeOnOutsideClick, say, vrMenu } from "./dom";
import { showChrome } from "./window";

const about = $<HTMLDialogElement>("about");
const notices = $<HTMLIFrameElement>("about-notices");
let aboutReturn: HTMLElement | null = null;

function openLink(e: MouseEvent) {
  const a = (e.target as Element | null)?.closest?.<HTMLAnchorElement>("a[href]");
  if (!a) return;
  e.preventDefault();
  openUrl(a.href).catch((err) => say(String(err)));
}

async function showAbout() {
  if (about.open) return;
  aboutReturn = document.activeElement as HTMLElement | null;
  closeDevices();
  vrMenu.hidden = true;
  about.showModal();
  showChrome();
  $("about-version").textContent = `Version ${await getVersion()}`;
  $("about-ffmpeg").textContent = await invoke<{ version: string; licence: string }>("ffmpeg_about").then(
    (f) => `FFmpeg ${f.version} · ${f.licence}`,
    () => "FFmpeg not found",
  );
}

closeOnOutsideClick(about, openLink);
about.addEventListener("close", () => {
  notices.hidden = true;
  about.classList.remove("notices");
  $("about-licenses").setAttribute("aria-expanded", "false");
  aboutReturn?.focus();
});
$("about-close").addEventListener("click", () => about.close());
$("about-licenses").addEventListener("click", async () => {
  const show = !about.classList.contains("notices");
  if (show && !notices.srcdoc) {
    // Vite's dev server answers a missing file with the app page itself.
    const page = await fetch("/licenses.html").then((r) => (r.ok ? r.text() : ""), () => "");
    notices.srcdoc = page.includes("<title>Third-party licences</title>")
      ? page
      : `<p style="font: 13px system-ui; color: #888">The licence list is generated when the app is built.</p>`;
  }
  notices.hidden = !show;
  about.classList.toggle("notices", show);
  $("about-licenses").setAttribute("aria-expanded", String(show));
});
notices.addEventListener("load", () => notices.contentDocument?.addEventListener("click", openLink));
for (const id of ["empty-about", "bar-about"]) $(id).addEventListener("click", showAbout);
listen("about", showAbout);
