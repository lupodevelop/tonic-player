// What the seek bar shows besides the position: chapter ticks, buffered
// ranges and the preview card.
import { $, drawRanges, player } from "./dom";
import { clock, fraction, previewLeft, thumbTime } from "./format";
import { app, ui } from "./state";

export function drawTicks() {
  const chapters = app.current?.chapters ?? [];
  const duration = ui.duration || chapters[chapters.length - 1]?.end || 0;
  $("ticks").replaceChildren(
    ...chapters
      .filter((c) => c.start > 0 && duration)
      .map((c) => {
        const tick = document.createElement("span");
        tick.style.left = `${(c.start / duration) * 100}%`;
        return tick;
      }),
  );
}
player.addEventListener("durationchange", drawTicks);

// What the player itself holds, from the video element.
function drawBuffered() {
  if (app.casting || !ui.duration) return drawRanges($("buffered"), []);
  const ranges: [number, number][] = [];
  for (let i = 0; i < player.buffered.length; i++) {
    ranges.push([player.buffered.start(i) / ui.duration, player.buffered.end(i) / ui.duration]);
  }
  drawRanges($("buffered"), ranges);
}
for (const event of ["progress", "timeupdate", "seeked", "emptied"]) player.addEventListener(event, drawBuffered);

// Seek bar preview: the frame under the pointer (extracted by ffmpeg on the
// stream server), its time and chapter. Requests go out once the pointer
// rests, so ffmpeg is not run for every pixel.
const scrub = $<HTMLDivElement>("scrub");
const preview = $<HTMLDivElement>("preview");
const previewImg = preview.querySelector("img")!;
let previewTimer = 0;

const chapterAt = (t: number) => app.current?.chapters.find((c) => t >= c.start && t < c.end)?.title ?? "";

previewImg.addEventListener("load", () => (previewImg.hidden = false));
previewImg.addEventListener("error", () => (previewImg.hidden = true));

scrub.addEventListener("mousemove", (e) => {
  const current = app.current;
  if (!current || !ui.duration) return;
  const box = scrub.getBoundingClientRect();
  const t = fraction(e.clientX, box.left, box.width) * ui.duration;
  // Keep the 176 px card inside the window, however short the bar is.
  preview.style.left = `${previewLeft(e.clientX, box.left, window.innerWidth)}px`;
  preview.querySelector(".when")!.textContent = clock(t);
  preview.querySelector(".chapter")!.textContent = chapterAt(t);
  preview.hidden = false;
  const at = thumbTime(t, ui.duration);
  const url = `http://127.0.0.1:${current.port}${current.thumbs.replace("{t}", String(at))}`;
  clearTimeout(previewTimer);
  previewTimer = window.setTimeout(() => {
    if (previewImg.dataset.url === url) return;
    previewImg.dataset.url = url;
    // Hidden until the new frame is in, so an older one never shows.
    previewImg.hidden = true;
    previewImg.src = url;
  }, 120);
});

scrub.addEventListener("mouseleave", () => {
  clearTimeout(previewTimer);
  preview.hidden = true;
});

export function resetPreview() {
  clearTimeout(previewTimer);
  preview.hidden = true;
  previewImg.hidden = true;
  previewImg.removeAttribute("src");
  previewImg.dataset.url = "";
}
