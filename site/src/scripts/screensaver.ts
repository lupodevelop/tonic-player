import { every, isPaused, reducedMotion, setPaused } from './loop';
import { ART_H, ART_W, Effect, Painter, loadFx, newFx, type Fx } from './fx';

const HOLD = [4, 4, 4, 0, 10];

/**
 * Full-screen effects. The grid spans the viewport width; effects cycle when
 * each one finishes, unless the visitor picks one. Under reduced motion each
 * effect is shown as a still of its finished frame.
 */
export function startScreensaver() {
  const saver = document.getElementById('saver')!;
  const stage = saver.querySelector<HTMLElement>('.stagefs')!;
  const canvas = stage.querySelector('canvas')!;
  const ui = saver.querySelector<HTMLElement>('.ui')!;
  const picks = [...saver.querySelectorAll<HTMLButtonElement>('[data-fx]')];
  const cycleBtn = saver.querySelector<HTMLButtonElement>('.cycle')!;
  const painter = new Painter(canvas, ART_W + 4, ART_H + 12);
  let fx: Fx | null = null;
  let effect: Effect = Effect.Fizz;
  let cycle = true;
  let held = 0;
  let yaw = 0;
  let pitch = -0.3;

  const build = () => {
    const rows = ART_H + 12;
    const px = Math.min(22, (innerHeight * 0.9) / (rows * 1.12));
    painter.rows = rows;
    painter.cols = Math.max(ART_W + 4, Math.floor(innerWidth / (px * 0.61)) - 1);
    fx = newFx(painter.cols, painter.rows, effect);
    fx.set_view(yaw, pitch);
    painter.fit(innerWidth, innerHeight * 0.9, 22);
    still();
  };
  const still = () => {
    if (!fx || !(reducedMotion || isPaused())) return;
    if (reducedMotion) for (let i = 0; i < 600 && !fx.done(); i++) fx.step(0.1);
    painter.paint(fx);
  };
  const choose = (e: Effect, byHand: boolean) => {
    effect = e;
    held = 0;
    if (byHand) setCycle(false);
    picks.forEach((b, i) => b.setAttribute('aria-pressed', String(i === e)));
    fx?.recap();
    fx?.set_effect(e);
    still();
  };
  const setCycle = (on: boolean) => {
    cycle = on;
    cycleBtn.setAttribute('aria-pressed', String(on));
  };

  picks.forEach((b, i) => b.addEventListener('click', (ev) => (ev.stopPropagation(), choose(i as Effect, true))));
  cycleBtn.addEventListener('click', (ev) => (ev.stopPropagation(), setCycle(!cycle)));

  let idle = 0;
  const wake = () => {
    ui.classList.remove('idle');
    clearTimeout(idle);
    idle = window.setTimeout(() => ui.classList.add('idle'), 2500);
  };
  addEventListener('pointermove', wake);
  wake();

  let drag: { x: number; y: number } | null = null;
  canvas.addEventListener('pointerdown', (e) => {
    drag = { x: e.clientX, y: e.clientY };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener('pointerup', (e) => {
    const tap = drag && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 4;
    drag = null;
    if (tap && effect !== Effect.Sphere && !reducedMotion) fx?.pop();
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!fx) return;
    if (drag && effect === Effect.Sphere) {
      yaw -= (e.clientX - drag.x) * 0.01;
      pitch = Math.max(-1.4, Math.min(1.4, pitch + (e.clientY - drag.y) * 0.01));
      drag = { x: e.clientX, y: e.clientY };
      fx.set_view(yaw, pitch);
      if (reducedMotion || isPaused()) (fx.step(0), painter.paint(fx));
    } else if (!reducedMotion) {
      const [x, y] = painter.cellAt(e.clientX, e.clientY);
      fx.poke(x, y, 3);
    }
  });

  addEventListener('keydown', (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key >= '1' && e.key <= '5') choose((Number(e.key) - 1) as Effect, true);
    else if (e.key === 'f' || e.key === 'F') {
      if (document.fullscreenElement) document.exitFullscreen();
      else saver.requestFullscreen?.().catch(() => undefined);
    } else if (e.key === ' ' && !(e.target as HTMLElement).closest('button, a')) {
      e.preventDefault();
      setPaused(!isPaused());
    } else return;
    wake();
  });

  let resizeTimer = 0;
  addEventListener('resize', () => {
    clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(build, 150);
  });

  loadFx()
    .then(() => {
      build();
      stage.classList.add('live');
      if (reducedMotion) return;
      every((dt) => {
        fx!.step(dt);
        painter.paint(fx!);
        if (!cycle || !fx!.done()) return;
        held += dt;
        if (held > HOLD[effect]) choose(((effect + 1) % 5) as Effect, false);
      });
    })
    .catch(() => undefined);
}
