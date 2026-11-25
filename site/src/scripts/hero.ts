import { every, reducedMotion } from './loop';
import { ART_H, ART_W, Effect, Painter, loadFx, newFx } from './fx';

const REST = 0.6;
const HOVER = 4;

export function startHero() {
  guessOs();
  if (!reducedMotion) bottle();
}

/**
 * Swaps the static bottle for the canvas once WASM is ready: the bottle fizzes
 * into shape once, then a few slow bubbles rise in the glass, more on hover.
 * The canvas takes the size of the static drawing so nothing shifts.
 */
function bottle() {
  const box = document.getElementById('bottle')!;
  const pre = box.querySelector('pre')!;
  const canvas = box.querySelector('canvas')!;
  const painter = new Painter(canvas, ART_W + 2, ART_H);
  loadFx()
    .then(() => {
      const fx = newFx(painter.cols, painter.rows, Effect.Fizz);
      fx.set_sparkle(false);
      fx.set_ambient(REST);
      const fit = () => {
        const r = pre.getBoundingClientRect();
        painter.fit((r.width * (ART_W + 2)) / ART_W, r.height, 40);
        painter.paint(fx);
      };
      fit();
      addEventListener('resize', fit);
      box.classList.add('live');
      box.addEventListener('pointerenter', () => fx.set_ambient(HOVER));
      box.addEventListener('pointerleave', () => fx.set_ambient(REST));
      every((dt) => {
        fx.step(dt);
        painter.paint(fx);
      }, box);
    })
    .catch(() => undefined);
}

/** Marks the download tile of the visitor's system. */
function guessOs() {
  const ua = `${(navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ?? ''} ${navigator.userAgent}`;
  if (/iPhone|iPad|Android/i.test(ua)) return;
  const os = /Mac/i.test(ua) ? 'mac' : /Win/i.test(ua) ? 'win' : /Linux|X11/i.test(ua) ? 'linux' : null;
  if (!os) return;
  document.querySelector(`.system[data-os="${os}"]`)?.classList.add('mine');
}
