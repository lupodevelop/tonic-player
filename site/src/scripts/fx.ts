import init, { Fx } from '../fx-pkg/tonic_fx.js';
import art from '../../fx/art/bottle.txt?raw';
import mask from '../../fx/art/bottle.mask.txt?raw';

export { Fx };
export const ART = art;
export const MASK = mask;
export const ART_W = Math.max(...art.trimEnd().split('\n').map((l) => [...l].length));
export const ART_H = art.trimEnd().split('\n').length;
export const Effect = { Fizz: 0, Decode: 1, Pour: 2, Spin: 3, Sphere: 4 } as const;
export type Effect = (typeof Effect)[keyof typeof Effect];

// Index = palette entry written by the engine.
const PALETTE = ['', '#faf1de', '#d1f30f', '#e4fa43', '#98cc04', '#3d7a1e', '#fffbe9', '#5f8f3c'];
const FULL = 0x2588;
const UPPER = 0x2580;
const FONT = '"JetBrains Mono", ui-monospace, monospace';

let ready: Promise<void> | null = null;

/** Loads the WASM engine once; rejects if the browser cannot run it. */
export function loadFx(): Promise<void> {
  ready ??= document.fonts
    .load(`16px ${FONT}`)
    .catch(() => undefined)
    .then(() => init())
    .then(() => undefined);
  return ready;
}

export function newFx(cols: number, rows: number, effect: Effect, seed = (Math.random() * 2 ** 32) >>> 0) {
  return new Fx(art, mask, cols, rows, effect, seed || 1);
}

/**
 * Paints engine frames on a canvas as a monospace grid. Block glyphs are
 * filled as rectangles so rows join without gaps; everything else is text.
 */
export class Painter {
  readonly ctx: CanvasRenderingContext2D;
  cw = 0;
  ch = 0;
  px = 0;

  constructor(
    readonly canvas: HTMLCanvasElement,
    public cols: number,
    public rows: number,
  ) {
    this.ctx = canvas.getContext('2d')!;
  }

  /** Sizes the canvas so the grid fits in `maxW` x `maxH` CSS pixels. */
  fit(maxW: number, maxH: number, maxPx = 18) {
    const m = this.measure(100);
    this.px = Math.max(4, Math.min(maxPx, (maxW / (this.cols * m)) * 100, maxH / (this.rows * 1.12)));
    this.cw = this.measure(this.px);
    this.ch = Math.ceil(this.px * 1.12);
    const dpr = Math.min(devicePixelRatio || 1, 2);
    const w = Math.ceil(this.cols * this.cw);
    const h = this.rows * this.ch;
    this.canvas.width = Math.ceil(w * dpr);
    this.canvas.height = Math.ceil(h * dpr);
    this.canvas.style.width = `${w}px`;
    this.canvas.style.height = `${h}px`;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.ctx.textBaseline = 'middle';
    this.ctx.textAlign = 'center';
    this.ctx.font = `${this.px}px ${FONT}`;
  }

  private measure(px: number) {
    this.ctx.font = `${px}px ${FONT}`;
    return this.ctx.measureText('M').width;
  }

  /** Converts a point in client coordinates to a grid cell. */
  cellAt(clientX: number, clientY: number): [number, number] {
    const r = this.canvas.getBoundingClientRect();
    return [Math.floor(((clientX - r.left) / r.width) * this.cols), Math.floor(((clientY - r.top) / r.height) * this.rows)];
  }

  paint(fx: Fx) {
    const { ctx, cw, ch, cols } = this;
    const chars = fx.chars();
    const colors = fx.colors();
    ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    for (let i = 0; i < chars.length; i++) {
      const c = chars[i];
      if (c === 32 || !colors[i]) continue;
      const x = (i % cols) * cw;
      const y = Math.floor(i / cols) * ch;
      ctx.fillStyle = PALETTE[colors[i]];
      if (c === FULL) ctx.fillRect(x, y, cw + 0.5, ch + 0.5);
      else if (c === UPPER) ctx.fillRect(x, y, cw + 0.5, ch / 2);
      else ctx.fillText(String.fromCodePoint(c), x + cw / 2, y + ch / 2);
    }
  }
}
