use std::f32::consts::{PI, TAU};
use wasm_bindgen::prelude::*;

pub const NONE: u8 = 0;
pub const CREAM: u8 = 1;
pub const LIME: u8 = 2;
pub const HIGH: u8 = 3;
pub const DARK: u8 = 4;
pub const GLASS: u8 = 5;
pub const FOAM: u8 = 6;
pub const DIM: u8 = 7;

const CAP_ROWS: usize = 4;
const POUR_RATE: f32 = 10.0;
const SPIN_RATE: f32 = 1.2;
const GRAVITY: f32 = 22.0;
const CELL_ASPECT: f32 = 0.55;
const NOISE: &[u8] = b"!<>-_\\/[]{}=+*^?#%&@01";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Fizz,
    Decode,
    Pour,
    Spin,
    Sphere,
}

impl From<u8> for Effect {
    fn from(n: u8) -> Self {
        [Effect::Fizz, Effect::Decode, Effect::Pour, Effect::Spin][..]
            .get(n as usize)
            .copied()
            .unwrap_or(Effect::Sphere)
    }
}

/// Width (longest line, in chars) and height (line count) of a block of text.
pub fn art_size(art: &str) -> (usize, usize) {
    let w = art.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    (w, art.lines().count())
}

fn color_of(m: char) -> u8 {
    match m {
        'c' => CREAM,
        'l' => LIME,
        'h' => HIGH,
        'd' => DARK,
        'g' => GLASS,
        _ => NONE,
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// A glyph travelling from (x0, y0), released at t0, rising at `speed` rows/s
/// towards its home cell (tx, ty); it drifts onto the home column as it nears.
struct Bubble {
    tx: usize,
    ty: usize,
    x0: f32,
    y0: f32,
    t0: f32,
    speed: f32,
    phase: f32,
}

impl Bubble {
    fn home(tx: usize, ty: usize) -> Self {
        Bubble { tx, ty, x0: tx as f32, y0: ty as f32, t0: 0.0, speed: 1.0, phase: 0.0 }
    }
    fn height(&self, t: f32) -> f32 {
        self.y0 - self.speed * (t - self.t0).max(0.0) - self.ty as f32
    }
    fn settled(&self, t: f32) -> bool {
        t >= self.t0 && self.height(t) <= 0.0
    }
    fn pos(&self, t: f32) -> (f32, f32, f32) {
        let d = self.height(t);
        let k = (d / 8.0).clamp(0.0, 1.0);
        let x = self.tx as f32 + (self.x0 - self.tx as f32) * k + (t * 6.0 + self.phase).sin() * 0.8 * k;
        (x, self.ty as f32 + d, d)
    }
}

/// A bubble drifting up through the liquid and glass of the finished bottle.
struct Drift {
    x: usize,
    y: f32,
    speed: f32,
}

/// A ballistic glyph: a flying cap piece or a fleck of foam.
struct Free {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    ch: char,
    col: u8,
}

/// Character-grid animation engine. The art is placed centred and bottom
/// aligned in a `w` x `h` grid; each `step` advances time and redraws `chars`
/// and `colors`, one entry per cell, row major.
pub struct Engine {
    w: usize,
    h: usize,
    ox: usize,
    oy: usize,
    aw: usize,
    ah: usize,
    target_ch: Vec<char>,
    target_col: Vec<u8>,
    out_ch: Vec<char>,
    out_col: Vec<u8>,
    cells: Vec<usize>,
    bubbles: Vec<Bubble>,
    reveal: Vec<f32>,
    effect: Effect,
    t: f32,
    rng: Rng,
    sparkle: bool,
    sparkles: Vec<(usize, f32)>,
    free: Vec<Free>,
    popped: bool,
    foam_left: f32,
    yaw: f32,
    pitch: f32,
    ambient: f32,
    drift: Vec<Drift>,
}

fn in_glass(col: u8) -> bool {
    matches!(col, GLASS | LIME | DARK | HIGH)
}

impl Engine {
    pub fn new(art: &str, mask: &str, w: usize, h: usize, effect: Effect, seed: u32) -> Result<Self, String> {
        let (aw, ah) = art_size(art);
        if art_size(mask) != (aw, ah) {
            return Err("art and mask differ in size".into());
        }
        if w < aw || h < ah {
            return Err(format!("grid {w}x{h} smaller than art {aw}x{ah}"));
        }
        let (ox, oy) = ((w - aw) / 2, h - ah);
        let mut target_ch = vec![' '; w * h];
        let mut target_col = vec![NONE; w * h];
        for (r, (line, mline)) in art.lines().zip(mask.lines()).enumerate() {
            let mut m = mline.chars();
            for (c, ch) in line.chars().enumerate() {
                let col = color_of(m.next().unwrap_or(' '));
                if (ch == ' ') != (col == NONE) {
                    return Err(format!("mask does not cover art at row {r}, column {c}"));
                }
                target_ch[(oy + r) * w + ox + c] = ch;
                target_col[(oy + r) * w + ox + c] = col;
            }
        }
        let cells: Vec<usize> = (0..w * h).filter(|&i| target_ch[i] != ' ').collect();
        let mut e = Engine {
            w,
            h,
            ox,
            oy,
            aw,
            ah,
            out_ch: target_ch.clone(),
            out_col: target_col.clone(),
            target_ch,
            target_col,
            cells,
            bubbles: Vec::new(),
            reveal: Vec::new(),
            effect,
            t: 0.0,
            rng: Rng(seed.max(1)),
            sparkle: true,
            sparkles: Vec::new(),
            free: Vec::new(),
            popped: false,
            foam_left: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            ambient: 0.0,
            drift: Vec::new(),
        };
        e.set_effect(effect);
        Ok(e)
    }

    /// Restarts time and lays out the starting state of `effect`.
    pub fn set_effect(&mut self, effect: Effect) {
        self.effect = effect;
        self.t = 0.0;
        self.sparkles.clear();
        let (w, h, aw) = (self.w, self.h as f32, self.aw.max(1) as f32);
        let mut bubbles = Vec::with_capacity(self.cells.len());
        let mut reveal = Vec::with_capacity(self.cells.len());
        for &i in &self.cells {
            let (tx, ty) = (i % w, i / w);
            let mut b = Bubble::home(tx, ty);
            if effect == Effect::Fizz {
                b.x0 = tx as f32 + self.rng.range(-4.0, 4.0);
                b.y0 = h + self.rng.range(0.0, 4.0);
                b.t0 = self.rng.range(0.0, 0.5);
                b.speed = self.rng.range(36.0, 60.0);
                b.phase = self.rng.range(0.0, TAU);
            }
            bubbles.push(b);
            reveal.push(0.4 + (tx - self.ox) as f32 / aw * 1.6 + self.rng.range(0.0, 1.0));
        }
        self.bubbles = bubbles;
        self.reveal = reveal;
        self.render();
    }

    pub fn set_sparkle(&mut self, on: bool) {
        self.sparkle = on;
        self.sparkles.clear();
    }

    /// Sets how many slow bubbles per second rise inside the finished bottle (0 for none).
    pub fn set_ambient(&mut self, per_second: f32) {
        self.ambient = per_second.max(0.0);
        if self.ambient == 0.0 {
            self.drift.clear();
        }
    }

    pub fn set_view(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-1.4, 1.4);
    }

    /// Advances time by `dt` seconds (non-finite or negative counts as 0,
    /// capped at 0.1 so a stalled tab does not skip the animation) and redraws.
    pub fn step(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        self.t += dt;
        for f in &mut self.free {
            f.x += f.vx * dt;
            f.y += f.vy * dt;
            f.vy += GRAVITY * dt;
        }
        let (w, h) = (self.w as f32, self.h as f32);
        self.free.retain(|f| f.y < h + 1.0 && f.x > -8.0 && f.x < w + 8.0);
        if self.foam_left > 0.0 {
            self.foam_left -= dt;
            let neck = (self.ox + self.aw / 2) as f32;
            let lip = (self.oy + CAP_ROWS) as f32;
            for _ in 0..(dt * 60.0).ceil() as usize {
                let ch = b"@oO*."[self.rng.next() as usize % 5] as char;
                self.free.push(Free {
                    x: neck + self.rng.range(-5.0, 5.0),
                    y: lip,
                    vx: self.rng.range(-9.0, 9.0),
                    vy: -self.rng.range(6.0, 16.0),
                    ch,
                    col: FOAM,
                });
            }
        }
        let w = self.w;
        for d in &mut self.drift {
            d.y -= d.speed * dt;
        }
        self.drift.retain(|d| d.y >= 0.0 && in_glass(self.target_col[(d.y.round() as usize) * w + d.x]));
        if self.ambient > 0.0 && self.done() && self.rng.unit() < self.ambient * dt {
            let i = self.cells[self.rng.next() as usize % self.cells.len()];
            if matches!(self.target_col[i], LIME | DARK) {
                self.drift.push(Drift { x: i % w, y: (i / w) as f32, speed: self.rng.range(1.5, 3.5) });
            }
        }
        self.sparkles.retain_mut(|s| {
            s.1 -= dt;
            s.1 > 0.0
        });
        if self.sparkle && self.done() && self.rng.unit() < dt * 4.0 {
            let i = self.cells[self.rng.next() as usize % self.cells.len()];
            if matches!(self.target_col[i], LIME | DARK | HIGH) {
                self.sparkles.push((i, 0.25));
            }
        }
        self.render();
    }

    /// True once the current effect has drawn the finished art and nothing is in flight.
    pub fn done(&self) -> bool {
        let t = self.t;
        let settled = || self.bubbles.iter().all(|b| b.settled(t));
        !self.popped
            && match self.effect {
                Effect::Fizz => settled(),
                Effect::Decode => self.reveal.iter().all(|r| t >= *r) && settled(),
                Effect::Pour => self.pour_level() <= self.oy as f32 - 1.0 && settled(),
                Effect::Spin => t >= TAU / SPIN_RATE,
                Effect::Sphere => true,
            }
    }

    /// Lifts settled cells within `r` of (x, y) off the art as bubbles that rise back home.
    pub fn poke(&mut self, x: i32, y: i32, r: i32) {
        if matches!(self.effect, Effect::Spin | Effect::Sphere) {
            return;
        }
        let t = self.t;
        for b in &mut self.bubbles {
            let (dx, dy) = (b.tx as i32 - x, b.ty as i32 - y);
            if dx * dx + dy * dy <= r * r && b.settled(t) {
                b.x0 = b.tx as f32 + self.rng.range(-3.0, 3.0);
                b.y0 = b.ty as f32 + self.rng.range(3.0, 8.0);
                b.t0 = t;
                b.speed = self.rng.range(8.0, 16.0);
                b.phase = self.rng.range(0.0, TAU);
            }
        }
    }

    /// Blows the cap off: its cells fly away and foam spills from the neck for a while.
    pub fn pop(&mut self) {
        if self.popped {
            return;
        }
        self.popped = true;
        self.foam_left = 2.5;
        let centre = (self.ox + self.aw / 2) as f32;
        let cap_end = (self.oy + CAP_ROWS) * self.w;
        for &i in self.cells.iter().filter(|&&i| i < cap_end) {
            let (x, y) = ((i % self.w) as f32, (i / self.w) as f32);
            self.free.push(Free {
                x,
                y,
                vx: (x - centre) * 0.8 + self.rng.range(-3.0, 3.0),
                vy: -self.rng.range(18.0, 28.0),
                ch: self.target_ch[i],
                col: CREAM,
            });
        }
        self.render();
    }

    /// Puts the cap back and replays the current effect.
    pub fn recap(&mut self) {
        self.popped = false;
        self.foam_left = 0.0;
        self.free.clear();
        self.set_effect(self.effect);
    }

    fn pour_level(&self) -> f32 {
        (self.oy + self.ah) as f32 - self.t * POUR_RATE
    }

    fn put(&mut self, x: f32, y: f32, ch: char, col: u8) {
        let (x, y) = (x.round(), y.round());
        if x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = y as usize * self.w + x as usize;
            self.out_ch[i] = ch;
            self.out_col[i] = col;
        }
    }

    fn render(&mut self) {
        self.out_ch.fill(' ');
        self.out_col.fill(NONE);
        match self.effect {
            Effect::Sphere => return self.render_sphere(),
            Effect::Spin => self.render_spin(),
            _ => {
                self.render_build();
                for k in 0..self.drift.len() {
                    let d = &self.drift[k];
                    let ch = if d.speed > 2.5 { 'o' } else { '°' };
                    let (x, y) = (d.x as f32, d.y);
                    self.put(x, y, ch, HIGH);
                }
            }
        }
        for k in 0..self.free.len() {
            let f = &self.free[k];
            let (x, y, ch, col) = (f.x, f.y, f.ch, f.col);
            self.put(x, y, ch, col);
        }
        for k in 0..self.sparkles.len() {
            let i = self.sparkles[k].0;
            let ch = if self.sparkles[k].1 > 0.12 { '*' } else { '+' };
            self.out_ch[i] = ch;
            self.out_col[i] = FOAM;
        }
    }

    fn render_build(&mut self) {
        let t = self.t;
        let level = self.pour_level();
        let cap_end = if self.popped { (self.oy + CAP_ROWS) * self.w } else { 0 };
        for k in 0..self.cells.len() {
            let i = self.cells[k];
            if i < cap_end || !self.bubbles[k].settled(t) {
                continue;
            }
            let (ch, col) = match self.effect {
                Effect::Decode if t < self.reveal[k] => (NOISE[self.rng.next() as usize % NOISE.len()] as char, DIM),
                Effect::Pour => {
                    let row = (i / self.w) as f32;
                    if row < level.floor() {
                        continue;
                    } else if row < level.ceil() || row == level {
                        ('~', HIGH)
                    } else {
                        (self.target_ch[i], self.target_col[i])
                    }
                }
                _ => (self.target_ch[i], self.target_col[i]),
            };
            self.out_ch[i] = ch;
            self.out_col[i] = col;
        }
        if self.effect == Effect::Pour && level > self.oy as f32 {
            let x = (self.ox + self.aw / 2) as f32;
            let mut y = 0.0;
            while y < level.floor() {
                self.put(x + (y * 0.7 + t * 9.0).sin() * 0.4, y, '|', HIGH);
                y += 1.0;
            }
        }
        for k in 0..self.bubbles.len() {
            let b = &self.bubbles[k];
            if b.settled(t) || t < b.t0 {
                continue;
            }
            let (x, y, d) = b.pos(t);
            let ch = if d > 10.0 { '.' } else if d > 4.0 { 'o' } else { 'O' };
            self.put(x, y, ch, HIGH);
        }
    }

    /// Turns the bottle about its vertical axis. Each row is treated as a
    /// circle with the row's half width as radius, so the silhouette stays put
    /// while the surface (label, bubbles) slides round; the far side shows
    /// each material without its details.
    fn render_spin(&mut self) {
        let a = self.t * SPIN_RATE;
        for r in 0..self.ah {
            let y = self.oy + r;
            let row = &self.target_ch[y * self.w..(y + 1) * self.w];
            let (Some(first), Some(last)) = (row.iter().position(|c| *c != ' '), row.iter().rposition(|c| *c != ' ')) else {
                continue;
            };
            if self.popped && r < CAP_ROWS {
                continue;
            }
            let cx = (first + last) as f32 / 2.0;
            let rad = ((last - first) as f32 / 2.0).max(0.5);
            for x in first..=last {
                let u = ((x as f32 - cx) / rad).clamp(-1.0, 1.0);
                let mut th = u.asin() + a;
                th = (th + PI).rem_euclid(TAU) - PI;
                let front = th.cos() >= 0.0;
                let sx = if front { cx + rad * th.sin() } else { cx - rad * th.sin() };
                // A turning cylinder keeps its outline: the edge columns never move, so
                // rounding in sin/asin cannot open a hole in the silhouette.
                let col_x = if x == first || x == last { x } else { (sx.round() as usize).clamp(first, last) };
                let src = y * self.w + col_x;
                let (mut ch, mut col) = (self.target_ch[src], self.target_col[src]);
                if !front {
                    ch = match col {
                        LIME => '%',
                        DARK => '#',
                        HIGH => '~',
                        GLASS => '.',
                        _ => '█',
                    };
                    if col == NONE {
                        col = CREAM;
                    }
                }
                if ch == ' ' && (x == first || x == last) {
                    (ch, col) = (self.target_ch[y * self.w + x], self.target_col[y * self.w + x]);
                }
                self.out_ch[y * self.w + x] = ch;
                self.out_col[y * self.w + x] = col;
            }
        }
    }

    /// Draws a globe seen from inside-out like a 360° frame: a shaded sphere
    /// with a latitude/longitude grid, a horizon and a sun, turned by yaw/pitch.
    fn render_sphere(&mut self) {
        const RAMP: &[u8] = b".:-=+*#%@";
        let (w, h) = (self.w as f32, self.h as f32);
        let ry = (h / 2.0 - 0.5).min((w / 2.0 - 0.5) * CELL_ASPECT).max(1.0);
        let yaw = self.yaw + self.t * 0.25;
        let (sy, cy, sp, cp) = (yaw.sin(), yaw.cos(), self.pitch.sin(), self.pitch.cos());
        for y in 0..self.h {
            for x in 0..self.w {
                let px = (x as f32 + 0.5 - w / 2.0) * CELL_ASPECT / ry;
                let py = (y as f32 + 0.5 - h / 2.0) / ry;
                let rr = px * px + py * py;
                if rr > 1.0 {
                    continue;
                }
                let pz = (1.0 - rr).sqrt();
                let (vy, vz) = (-py * cp - pz * sp, -py * sp + pz * cp);
                let (vx, vz) = (px * cy + vz * sy, -px * sy + vz * cy);
                let lon = vx.atan2(vz).to_degrees();
                let lat = vy.clamp(-1.0, 1.0).asin().to_degrees();
                let on_lon = (lon.rem_euclid(30.0) - 15.0).abs() > 13.0;
                let on_lat = (lat.rem_euclid(30.0) - 15.0).abs() > 13.5;
                let light = (0.35 * pz - 0.45 * py - 0.3 * px + 0.55).clamp(0.0, 0.999);
                let (ch, col) = if (lon - 20.0).abs() < 9.0 && (lat - 32.0).abs() < 6.0 {
                    ('O', HIGH)
                } else if lat.abs() < 2.5 {
                    ('~', HIGH)
                } else if on_lon && on_lat {
                    ('+', CREAM)
                } else if on_lon {
                    ('|', LIME)
                } else if on_lat {
                    ('-', LIME)
                } else {
                    (RAMP[(light * RAMP.len() as f32) as usize] as char, if lat > 0.0 { GLASS } else { DARK })
                };
                let i = y * self.w + x;
                self.out_ch[i] = ch;
                self.out_col[i] = col;
            }
        }
    }

    pub fn chars(&self) -> &[char] {
        &self.out_ch
    }
    pub fn colors(&self) -> &[u8] {
        &self.out_col
    }
    pub fn target_chars(&self) -> &[char] {
        &self.target_ch
    }
    pub fn target_colors(&self) -> &[u8] {
        &self.target_col
    }
    pub fn width(&self) -> usize {
        self.w
    }
    pub fn height(&self) -> usize {
        self.h
    }
    pub fn art_origin(&self) -> (usize, usize) {
        (self.ox, self.oy)
    }
}

/// JavaScript handle on an `Engine`. `effect` is 0 fizz, 1 decode, 2 pour,
/// 3 spin, anything else sphere. Frames are read with `chars` (code points)
/// and `colors` (palette indices), both row major.
#[wasm_bindgen]
pub struct Fx(Engine);

#[wasm_bindgen]
impl Fx {
    #[wasm_bindgen(constructor)]
    pub fn new(art: &str, mask: &str, w: usize, h: usize, effect: u8, seed: u32) -> Result<Fx, JsError> {
        Engine::new(art, mask, w, h, effect.into(), seed).map(Fx).map_err(|e| JsError::new(&e))
    }
    pub fn step(&mut self, dt: f32) {
        self.0.step(dt)
    }
    pub fn chars(&self) -> Vec<u32> {
        self.0.chars().iter().map(|&c| c as u32).collect()
    }
    pub fn colors(&self) -> Vec<u8> {
        self.0.colors().to_vec()
    }
    pub fn width(&self) -> usize {
        self.0.width()
    }
    pub fn height(&self) -> usize {
        self.0.height()
    }
    pub fn done(&self) -> bool {
        self.0.done()
    }
    pub fn poke(&mut self, x: i32, y: i32, r: i32) {
        self.0.poke(x, y, r)
    }
    pub fn pop(&mut self) {
        self.0.pop()
    }
    pub fn recap(&mut self) {
        self.0.recap()
    }
    pub fn set_effect(&mut self, effect: u8) {
        self.0.set_effect(effect.into())
    }
    pub fn set_sparkle(&mut self, on: bool) {
        self.0.set_sparkle(on)
    }
    pub fn set_view(&mut self, yaw: f32, pitch: f32) {
        self.0.set_view(yaw, pitch)
    }
    pub fn set_ambient(&mut self, per_second: f32) {
        self.0.set_ambient(per_second)
    }
}

#[cfg(test)]
mod tests;
