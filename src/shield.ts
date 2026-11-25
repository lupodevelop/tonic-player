// Shield: the proxy for torrents. It is read once at launch, so a change
// restarts the app.
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { closeDevices } from "./cast";
import { $, closeOnOutsideClick, dialogOpen, vrMenu } from "./dom";
import { showChrome } from "./window";

type Shield = { on: boolean; proxy: string };
const dialog = $<HTMLDialogElement>("shield");
const proxyField = $<HTMLInputElement>("shield-proxy");
let shield: Shield = { on: false, proxy: "" };

function render() {
  for (const b of document.querySelectorAll(".shield-open")) {
    b.classList.toggle("on", shield.on);
    b.setAttribute("aria-label", shield.on ? "Shield on" : "Shield off");
  }
  const edited = proxyField.value.trim() !== shield.proxy;
  $("shield-status").textContent = shield.on ? "On: torrents go through the proxy." : "Off.";
  $("shield-apply").textContent = shield.on ? (edited ? "Save and restart" : "Turn off and restart") : "Turn on and restart";
}

function show() {
  if (dialogOpen()) return;
  closeDevices();
  vrMenu.hidden = true;
  proxyField.value = shield.proxy;
  render();
  dialog.showModal();
  showChrome();
}

proxyField.addEventListener("input", render);
$("shield-close").addEventListener("click", () => dialog.close());
closeOnOutsideClick(dialog);
$("shield-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const proxy = proxyField.value.trim();
  const on = !shield.on || proxy !== shield.proxy;
  try {
    await invoke("set_shield", { on, proxy });
    await relaunch();
  } catch (err) {
    $("shield-status").textContent = String(err);
  }
});
for (const b of document.querySelectorAll(".shield-open")) b.addEventListener("click", show);
invoke<Shield>("shield_state").then((s) => {
  shield = s;
  render();
});
