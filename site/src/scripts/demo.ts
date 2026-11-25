import { reducedMotion } from './loop';

const IDLE_MS = 2500;
const TIP_DELAY_MS = 300;
const THUMB_W = 168;
const THUMB_H = 94.5;

type Chapter = { start: number; title: string };

const clock = (t: number) => `${Math.floor(t / 60)}:${String(Math.floor(t % 60)).padStart(2, '0')}`;
const clamp01 = (x: number) => Math.min(1, Math.max(0, x)) || 0;

/**
 * Drives the replica of the app window. The clip plays muted and loops on its
 * own while the demo is on screen and the tab is visible, unless the visitor
 * paused it.
 * Like the app: the seek bar maps the pointer over its full width and shows a
 * frame preview with the chapter on hover, the chrome hides after 2.5 s without
 * pointer movement while playing, and the mini player floats in a corner.
 * Buttons for features a web page cannot have explain so in a tooltip.
 * Under reduced motion the window does not animate.
 */
export function startDemo(app: HTMLElement) {
  const $ = <T extends HTMLElement = HTMLElement>(sel: string) => app.querySelector<T>(sel)!;
  const slot = app.parentElement!;
  const video = $<HTMLVideoElement>('video');
  const bar = $('.bar');
  const toggle = $('.toggle');
  const scrub = $('.scrub');
  const played = $('.played');
  const buffered = $('.buffered');
  const knob = $('.knob');
  const now = $('.now');
  const total = $('.total');
  const preview = $('.preview');
  const thumb = $('.thumb');
  const audio = $('.audio');
  const volume = $('.volume');
  const tip = $('.tip');
  const miniBtn = $('.mini');
  const chapters: Chapter[] = JSON.parse(app.dataset.chapters ?? '[]');
  const step = Number(app.dataset.thumbStep);
  const cols = Number(app.dataset.thumbCols);
  thumb.style.backgroundImage = `url("${app.dataset.thumbs}")`;

  let onScreen = false;
  let userPaused = false;
  let dragging = false;
  let idleTimer = 0;
  let level = 0.8;
  let muted = false;

  const duration = () => (Number.isFinite(video.duration) ? video.duration : 0);

  const render = () => {
    const d = duration();
    const t = video.currentTime;
    const f = d ? t / d : 0;
    played.style.width = `${f * 100}%`;
    knob.style.left = `${f * 100}%`;
    const end = video.buffered.length ? video.buffered.end(video.buffered.length - 1) : 0;
    buffered.style.width = `${d ? (end / d) * 100 : 0}%`;
    now.textContent = clock(t);
    scrub.setAttribute('aria-valuenow', String(Math.round(t)));
    scrub.setAttribute('aria-valuetext', clock(t));
  };

  const wake = () => {
    app.classList.remove('idle');
    clearTimeout(idleTimer);
    idleTimer = window.setTimeout(() => {
      if (!video.paused && !dragging && !app.contains(document.activeElement)) app.classList.add('idle');
    }, IDLE_MS);
  };

  const sync = () => {
    bar.classList.toggle('paused', video.paused);
    toggle.setAttribute('aria-label', video.paused ? 'Play' : 'Pause');
    wake();
  };

  const play = () => video.play().catch(() => undefined);
  const update = () => {
    const visible = onScreen || app.classList.contains('mini') || !!document.fullscreenElement;
    if (visible && !document.hidden && !userPaused) play();
    else video.pause();
  };

  const flip = () => {
    userPaused = !video.paused;
    if (video.paused) play();
    else video.pause();
  };

  const seekBy = (s: number) => {
    if (!duration()) return;
    video.currentTime = Math.min(duration() - 0.1, Math.max(0, video.currentTime + s));
    render();
  };

  video.addEventListener('loadedmetadata', () => {
    total.textContent = clock(duration());
    scrub.setAttribute('aria-valuemax', String(Math.round(duration())));
    render();
  });
  video.addEventListener('timeupdate', render);
  video.addEventListener('progress', render);
  video.addEventListener('play', sync);
  video.addEventListener('pause', sync);

  toggle.addEventListener('click', flip);
  $('.back').addEventListener('click', () => seekBy(-10));
  $('.forward').addEventListener('click', () => seekBy(10));

  const fullscreen = () => {
    if (document.fullscreenElement) document.exitFullscreen();
    else app.requestFullscreen?.().catch(() => undefined);
  };
  $('.fullscreen').addEventListener('click', fullscreen);
  document.addEventListener('fullscreenchange', update);

  // Seek bar: full-width pointer mapping, drag to scrub, preview on hover.
  const fracAt = (x: number) => {
    const box = scrub.getBoundingClientRect();
    return clamp01((x - box.left) / box.width);
  };
  const showPreview = (x: number) => {
    const d = duration();
    if (!d || app.classList.contains('mini')) return;
    const t = fracAt(x) * d;
    const box = scrub.getBoundingClientRect();
    preview.style.left = `${Math.min(box.width - 80, Math.max(80, x - box.left))}px`;
    const i = Math.min(Math.round(t / step), Math.floor(d / step) - 1);
    thumb.style.backgroundPosition = `${-(i % cols) * THUMB_W}px ${-Math.floor(i / cols) * THUMB_H}px`;
    $('.chapter').textContent = [...chapters].reverse().find((c) => t >= c.start)?.title ?? '';
    $('.when').textContent = clock(t);
    preview.hidden = false;
  };
  scrub.addEventListener('pointermove', (e) => {
    if (e.pointerType === 'mouse' || dragging) showPreview(e.clientX);
    if (dragging) {
      video.currentTime = fracAt(e.clientX) * duration();
      render();
    }
  });
  scrub.addEventListener('pointerleave', () => !dragging && (preview.hidden = true));
  scrub.addEventListener('pointerdown', (e) => {
    if (!duration()) return;
    dragging = true;
    scrub.classList.add('dragging');
    scrub.setPointerCapture(e.pointerId);
    video.currentTime = fracAt(e.clientX) * duration();
    showPreview(e.clientX);
    render();
  });
  const drop = (e: PointerEvent) => {
    if (!dragging) return;
    dragging = false;
    scrub.classList.remove('dragging');
    if (e.pointerType !== 'mouse') preview.hidden = true;
    wake();
  };
  scrub.addEventListener('pointerup', drop);
  scrub.addEventListener('pointercancel', drop);
  scrub.addEventListener('keydown', (e) => {
    const s = { ArrowLeft: -5, ArrowRight: 5, Home: -Infinity, End: Infinity }[e.key];
    if (s === undefined) return;
    e.preventDefault();
    e.stopPropagation();
    seekBy(s);
  });

  // Volume: the clip is silent, so the slider and the mute button only move the UI.
  const setLevel = (v: number) => {
    level = clamp01(v);
    muted = level === 0;
    $('.volume-fill').style.width = `${level * 100}%`;
    $('.volume-knob').style.left = `calc(8px + (100% - 16px) * ${level})`;
    volume.setAttribute('aria-valuenow', String(Math.round(level * 100)));
    bar.classList.toggle('muted', muted);
  };
  const levelAt = (x: number) => {
    const track = volume.querySelector('.track')!.getBoundingClientRect();
    return (x - track.left) / track.width;
  };
  volume.addEventListener('pointerdown', (e) => {
    volume.setPointerCapture(e.pointerId);
    audio.classList.add('adjusting');
    setLevel(levelAt(e.clientX));
  });
  volume.addEventListener('pointermove', (e) => audio.classList.contains('adjusting') && setLevel(levelAt(e.clientX)));
  volume.addEventListener('pointerup', () => audio.classList.remove('adjusting'));
  volume.addEventListener('keydown', (e) => {
    const s = { ArrowLeft: -0.1, ArrowDown: -0.1, ArrowRight: 0.1, ArrowUp: 0.1 }[e.key];
    if (s === undefined) return;
    e.preventDefault();
    e.stopPropagation();
    setLevel(level + s);
  });
  $('.mute').addEventListener('click', () => {
    muted = !muted;
    bar.classList.toggle('muted', muted);
    $('.mute').setAttribute('aria-label', muted ? 'Unmute' : 'Mute');
  });

  // Buttons a web page cannot honour show a tooltip after a short delay.
  let tipTimer = 0;
  const showTip = (b: HTMLElement) => {
    const a = app.getBoundingClientRect();
    const r = b.getBoundingClientRect();
    tip.style.left = `${Math.min(a.width - 130, Math.max(130, r.left + r.width / 2 - a.left))}px`;
    tip.style.top = `${r.top - a.top - 8}px`;
    tip.hidden = false;
  };
  const hideTip = () => {
    clearTimeout(tipTimer);
    tip.hidden = true;
  };
  app.querySelectorAll<HTMLElement>('.nothere').forEach((b) => {
    const later = () => {
      clearTimeout(tipTimer);
      tipTimer = window.setTimeout(() => showTip(b), TIP_DELAY_MS);
    };
    b.addEventListener('pointerenter', later);
    b.addEventListener('focus', later);
    b.addEventListener('click', () => (clearTimeout(tipTimer), showTip(b)));
    b.addEventListener('pointerleave', hideTip);
    b.addEventListener('blur', hideTip);
  });

  // Mini player: the window moves to a fixed corner; the move is animated from
  // its old box to the new one (FLIP), except under reduced motion.
  const setMini = (on: boolean) => {
    if (app.classList.contains('mini') === on) return;
    hideTip();
    preview.hidden = true;
    const from = app.getBoundingClientRect();
    app.classList.toggle('mini', on);
    slot.classList.toggle('empty', on);
    miniBtn.setAttribute('aria-pressed', String(on));
    miniBtn.setAttribute('aria-label', on ? 'Back to the page' : 'Mini player');
    if (!reducedMotion) {
      const to = app.getBoundingClientRect();
      app.animate(
        [
          {
            transformOrigin: 'top left',
            transform: `translate(${from.left - to.left}px, ${from.top - to.top}px) scale(${from.width / to.width}, ${from.height / to.height})`,
          },
          { transformOrigin: 'top left', transform: 'none' },
        ],
        { duration: 420, easing: 'cubic-bezier(.2,.8,.2,1)' },
      );
    }
    update();
    wake();
  };
  miniBtn.addEventListener('click', () => setMini(!app.classList.contains('mini')));
  $('.mini-expand').addEventListener('click', () => setMini(false));
  slot.querySelector('.return')!.addEventListener('click', () => setMini(false));
  $('.mini-close').addEventListener('click', () => setMini(false));

  app.addEventListener('keydown', (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const onButton = !!(e.target as HTMLElement).closest('button');
    const k = e.key.toLowerCase();
    if (k === ' ' && !onButton) flip();
    else if (k === 'arrowleft' || k === 'j') seekBy(-10);
    else if (k === 'arrowright' || k === 'l') seekBy(10);
    else if (k === 'f') fullscreen();
    else if (k === 'p') setMini(!app.classList.contains('mini'));
    else if (k === 'escape' && app.classList.contains('mini')) setMini(false);
    else return;
    e.preventDefault();
    wake();
  });
  app.addEventListener('pointermove', wake);
  app.addEventListener('focusin', wake);

  new IntersectionObserver(
    ([entry]) => {
      onScreen = entry.isIntersecting;
      update();
    },
    { threshold: 0.4 },
  ).observe(slot);
  document.addEventListener('visibilitychange', update);
  addEventListener('blur', () => app.classList.add('inactive'));
  addEventListener('focus', () => app.classList.remove('inactive'));
  setLevel(level);
  render();
}
