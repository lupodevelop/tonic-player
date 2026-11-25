type Tick = (dt: number) => void;
type Job = { tick: Tick; visible: boolean };

const jobs = new Set<Job>();
const watch = new IntersectionObserver((entries) => {
  for (const e of entries) {
    const job = owners.get(e.target);
    if (job) job.visible = e.isIntersecting;
  }
});
const owners = new WeakMap<Element, Job>();

export const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
let paused = false;
let last = 0;
let raf = 0;

/**
 * Runs `tick` on the page's single animation frame loop. With `el`, the job
 * only runs while that element is on screen. Nothing runs while the tab is
 * hidden, while the page is paused, or at all under reduced motion.
 * Returns a function that removes the job.
 */
export function every(tick: Tick, el?: Element): () => void {
  const job: Job = { tick, visible: !el };
  if (el) {
    owners.set(el, job);
    watch.observe(el);
  }
  jobs.add(job);
  start();
  return () => {
    jobs.delete(job);
    if (el) watch.unobserve(el);
  };
}

function frame(now: number) {
  const dt = Math.min((now - last) / 1000, 0.1);
  last = now;
  for (const job of jobs) if (job.visible) job.tick(dt);
  raf = requestAnimationFrame(frame);
}

function start() {
  if (raf || reducedMotion || paused || document.hidden) return;
  last = performance.now();
  raf = requestAnimationFrame(frame);
}

function stop() {
  cancelAnimationFrame(raf);
  raf = 0;
}

export function isPaused() {
  return paused;
}

/** Pauses or resumes every job on the loop. */
export function setPaused(p: boolean) {
  paused = p;
  if (p) stop();
  else start();
}

document.addEventListener('visibilitychange', () => (document.hidden ? stop() : start()));
