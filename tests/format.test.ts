import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clock,
  fraction,
  miniBox,
  nextEpisode,
  previewLeft,
  rate,
  sameLayout,
  size,
  thumbTime,
  verticalWidth,
} from "../src/format.ts";

test("clock shows minutes, and hours only when there are some", () => {
  assert.equal(clock(0), "0:00");
  assert.equal(clock(59.9), "0:59");
  assert.equal(clock(61), "1:01");
  assert.equal(clock(3540), "59:00");
  assert.equal(clock(3599), "59:59");
  assert.equal(clock(3600), "1:00:00");
  assert.equal(clock(3725), "1:02:05");
  assert.equal(clock(-5), "0:00");
  assert.equal(clock(NaN), "0:00");
});

test("size rounds to MB below a gigabyte, never under 1 MB", () => {
  assert.equal(size(0), "1 MB");
  assert.equal(size(1_400_000), "1 MB");
  assert.equal(size(1_600_000), "2 MB");
  assert.equal(size(999_000_000), "999 MB");
  assert.equal(size(1e9), "1.0 GB");
  assert.equal(size(2_350_000_000), "2.4 GB");
});

test("rate switches from KB/s to MB/s at one megabyte", () => {
  assert.equal(rate(0), "0 KB/s");
  assert.equal(rate(1499), "1 KB/s");
  assert.equal(rate(999_999), "1000 KB/s");
  assert.equal(rate(1e6), "1.0 MB/s");
  assert.equal(rate(12_340_000), "12.3 MB/s");
});

test("layouts match on field of view and stereo", () => {
  assert.ok(sameLayout(null, null));
  assert.ok(sameLayout({ fov: 360, stereo: "tb" }, { fov: 360, stereo: "tb" }));
  assert.ok(!sameLayout({ fov: 360, stereo: "tb" }, { fov: 360, stereo: "sbs" }));
  assert.ok(!sameLayout({ fov: 180, stereo: "mono" }, { fov: 360, stereo: "mono" }));
  assert.ok(!sameLayout(null, { fov: 360, stereo: "mono" }));
});

test("fraction clamps the pointer to the box and survives an empty box", () => {
  assert.equal(fraction(150, 100, 200), 0.25);
  assert.equal(fraction(50, 100, 200), 0);
  assert.equal(fraction(400, 100, 200), 1);
  assert.equal(fraction(100, 100, 0), 0);
});

test("the preview card stays inside the window", () => {
  // Bar from x = 20, window 400 wide, card half width 96.
  assert.equal(previewLeft(200, 20, 400), 180);
  assert.equal(previewLeft(30, 20, 400), 76);
  assert.equal(previewLeft(390, 20, 400), 284);
});

test("the mini player's longer side is 480", () => {
  assert.deepEqual(miniBox(1920, 1080), { width: 480, height: 270 });
  assert.deepEqual(miniBox(1080, 1920), { width: 270, height: 480 });
  assert.deepEqual(miniBox(1000, 1000), { width: 480, height: 480 });
  assert.deepEqual(miniBox(1000, 999), { width: 480, height: 480 });
  assert.deepEqual(miniBox(0, 0), { width: 480, height: 270 });
  assert.deepEqual(miniBox(1920, 0), { width: 480, height: 270 });
});

test("a vertical video narrows the window to its shape at the same height", () => {
  assert.equal(verticalWidth(1000, 1080, 1920), 563);
  assert.equal(verticalWidth(800, 720, 1280), 450);
});

test("the next episode follows the playing one", () => {
  const list = [{ file_id: 3 }, { file_id: 7 }, { file_id: 9 }];
  assert.equal(nextEpisode(list, 3)?.file_id, 7);
  assert.equal(nextEpisode(list, 7)?.file_id, 9);
  assert.equal(nextEpisode(list, 9), undefined);
  assert.equal(nextEpisode(list, 5), undefined);
  assert.equal(nextEpisode(list, null), undefined);
  assert.equal(nextEpisode([], 3), undefined);
});

test("preview frames snap to even seconds inside the video", () => {
  assert.equal(thumbTime(0, 100), 0);
  assert.equal(thumbTime(2.9, 100), 2);
  assert.equal(thumbTime(3.1, 100), 4);
  assert.equal(thumbTime(99.5, 99.5), 99);
});
