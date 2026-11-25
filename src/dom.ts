// Elements several modules use, and the small helpers around them.
export const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

export const player = $<HTMLVideoElement>("player");
export const empty = $<HTMLButtonElement>("empty");
export const bar = $<HTMLDivElement>("bar");
export const toast = $<HTMLParagraphElement>("toast");
export const spinner = $("spinner");
export const chooser = $<HTMLElement>("chooser");
export const devices = $<HTMLUListElement>("devices");
export const vrMenu = $<HTMLUListElement>("vr-menu");

let toastTimer = 0;
export function say(text: string, sticky = false) {
  clearTimeout(toastTimer);
  toast.classList.remove("action");
  toast.textContent = text;
  toast.hidden = false;
  if (!sticky) toastTimer = window.setTimeout(() => (toast.hidden = true), 3500);
}

export function row(content: string | HTMLElement) {
  const li = document.createElement("li");
  li.append(content);
  return li;
}

export function dialogOpen() {
  return document.querySelector("dialog[open]") !== null;
}

/** Wires a modal dialog to close on a click outside its box; other clicks go to `rest`. */
export function closeOnOutsideClick(dialog: HTMLDialogElement, rest?: (e: MouseEvent) => void) {
  dialog.addEventListener("click", (e) => {
    const box = dialog.getBoundingClientRect();
    const outside = e.clientX < box.left || e.clientX > box.right || e.clientY < box.top || e.clientY > box.bottom;
    if (e.target === dialog && outside) return dialog.close();
    rest?.(e);
  });
}

/** Draws `(start, end)` fractions of the bar as segments inside `layer`. */
export function drawRanges(layer: HTMLElement, ranges: [number, number][]) {
  layer.replaceChildren(
    ...ranges.map(([a, b]) => {
      const span = document.createElement("span");
      span.style.left = `${a * 100}%`;
      span.style.width = `${(b - a) * 100}%`;
      return span;
    }),
  );
}
