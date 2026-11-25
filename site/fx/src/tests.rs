use super::*;

const ART: &str = include_str!("../art/bottle.txt");
const MASK: &str = include_str!("../art/bottle.mask.txt");

fn engine(effect: Effect, seed: u32) -> Engine {
    let (w, h) = art_size(ART);
    let mut e = Engine::new(ART, MASK, w + 6, h + 10, effect, seed).unwrap();
    e.set_sparkle(false);
    e
}

fn run_until_done(e: &mut Engine, limit: f32) -> f32 {
    let mut t = 0.0;
    while !e.done() && t < limit {
        e.step(1.0 / 30.0);
        t += 1.0 / 30.0;
    }
    t
}

fn frame(e: &Engine) -> (Vec<char>, Vec<u8>) {
    (e.chars().to_vec(), e.colors().to_vec())
}

#[test]
fn bundled_art_and_mask_have_the_same_shape() {
    assert_eq!(art_size(ART), art_size(MASK));
    assert!(Engine::new(ART, MASK, 10, 10, Effect::Fizz, 1).is_err(), "grid smaller than art");
    assert!(Engine::new("ab\n", "a\n", 10, 10, Effect::Fizz, 1).is_err(), "mask shape differs");
}

#[test]
fn fizz_starts_scattered_and_is_not_done() {
    let mut e = engine(Effect::Fizz, 7);
    e.step(0.0);
    assert!(!e.done());
    let matching = e
        .chars()
        .iter()
        .zip(e.target_chars())
        .filter(|(a, b)| **b != ' ' && a == b)
        .count();
    let total = e.target_chars().iter().filter(|c| **c != ' ').count();
    assert!(matching * 4 < total, "{matching} of {total} cells already in place");
}

#[test]
fn every_build_effect_settles_on_the_target() {
    for effect in [Effect::Fizz, Effect::Decode, Effect::Pour] {
        let mut e = engine(effect, 3);
        let t = run_until_done(&mut e, 60.0);
        assert!(e.done(), "{effect:?} never finished");
        assert!(t > 0.5, "{effect:?} finished instantly");
        e.step(1.0 / 30.0);
        assert_eq!(e.chars(), e.target_chars(), "{effect:?}");
        assert_eq!(e.colors(), e.target_colors(), "{effect:?}");
    }
}

#[test]
fn same_seed_gives_same_frames() {
    let (mut a, mut b, mut c) = (engine(Effect::Fizz, 9), engine(Effect::Fizz, 9), engine(Effect::Fizz, 10));
    for _ in 0..20 {
        a.step(0.05);
        b.step(0.05);
        c.step(0.05);
    }
    assert_eq!(frame(&a), frame(&b));
    assert_ne!(frame(&a), frame(&c));
}

#[test]
fn odd_time_steps_stay_in_bounds() {
    for effect in [Effect::Fizz, Effect::Decode, Effect::Pour, Effect::Spin, Effect::Sphere] {
        let mut e = engine(effect, 5);
        for dt in [0.0, 10.0, -1.0, f32::NAN, 1e9, 0.001] {
            e.step(dt);
            e.poke(-50, -50, 3);
            e.poke(1000, 1000, 3);
        }
        assert_eq!(e.chars().len(), e.width() * e.height());
    }
}

#[test]
fn poke_scatters_cells_that_then_settle_again() {
    let mut e = engine(Effect::Decode, 2);
    run_until_done(&mut e, 60.0);
    let mid = e.width() * (e.height() - 10);
    let i = (mid..e.width() * e.height()).find(|&i| e.target_chars()[i] != ' ').unwrap();
    e.poke((i % e.width()) as i32, (i / e.width()) as i32, 3);
    assert!(!e.done());
    e.step(0.02);
    assert_ne!(e.chars(), e.target_chars());
    run_until_done(&mut e, 30.0);
    e.step(1.0 / 30.0);
    assert_eq!(e.chars(), e.target_chars());
}

#[test]
fn pop_sends_the_cap_away_and_releases_foam() {
    let mut e = engine(Effect::Decode, 4);
    run_until_done(&mut e, 60.0);
    let cap_cells = |e: &Engine| {
        let (ox, oy) = e.art_origin();
        let aw = art_size(ART).0;
        (0..CAP_ROWS)
            .flat_map(move |r| (0..aw).map(move |c| (ox + c, oy + r)))
            .filter(|&(x, y)| e.chars()[y * e.width() + x] == e.target_chars()[y * e.width() + x] && e.target_chars()[y * e.width() + x] != ' ')
            .count()
    };
    let before = cap_cells(&e);
    assert!(before > 0);
    e.pop();
    for _ in 0..60 {
        e.step(1.0 / 30.0);
    }
    assert!(cap_cells(&e) < before / 4, "cap still on the bottle");
    assert!(e.colors().iter().any(|&c| c == FOAM), "no foam");
    e.recap();
    run_until_done(&mut e, 30.0);
    e.step(1.0 / 30.0);
    assert_eq!(e.chars(), e.target_chars());
}

#[test]
fn spin_keeps_the_silhouette_and_starts_facing_front() {
    let mut e = engine(Effect::Spin, 1);
    e.step(0.0);
    assert_eq!(e.chars(), e.target_chars());
    let extent = |cells: &[char], w: usize| -> Vec<Option<(usize, usize)>> {
        cells
            .chunks(w)
            .map(|row| {
                let first = row.iter().position(|c| *c != ' ')?;
                let last = row.iter().rposition(|c| *c != ' ')?;
                Some((first, last))
            })
            .collect()
    };
    let target = extent(e.target_chars(), e.width());
    e.step(0.7);
    assert_ne!(e.chars(), e.target_chars());
    assert_eq!(extent(e.chars(), e.width()), target);
}

#[test]
fn sphere_is_round_and_turns_with_the_view() {
    let mut e = engine(Effect::Sphere, 1);
    e.step(0.0);
    let (w, h) = (e.width(), e.height());
    let at = |e: &Engine, x: usize, y: usize| e.chars()[y * w + x];
    assert_ne!(at(&e, w / 2, h / 2), ' ');
    assert_eq!(at(&e, 0, 0), ' ');
    assert_eq!(at(&e, w - 1, h - 1), ' ');
    let before = frame(&e);
    e.set_view(1.0, 0.2);
    e.step(0.0);
    assert_ne!(frame(&e), before);
}

#[test]
fn spin_finishes_after_one_turn() {
    let mut e = engine(Effect::Spin, 1);
    let turn = TAU / SPIN_RATE;
    for _ in 0..(turn / 0.1) as usize - 1 {
        e.step(0.1);
    }
    assert!(!e.done());
    e.step(0.1);
    e.step(0.1);
    assert!(e.done());
}

#[test]
fn poke_only_lifts_cells_inside_the_radius() {
    let mut e = engine(Effect::Decode, 6);
    run_until_done(&mut e, 60.0);
    let w = e.width();
    let i = (w * (e.height() - 10)..w * e.height()).find(|&i| e.target_chars()[i] != ' ').unwrap();
    let (x, y) = ((i % w) as i32, (i / w) as i32);
    e.poke(x, y, 1);
    e.step(0.0);
    for (k, (a, b)) in e.chars().iter().zip(e.target_chars()).enumerate() {
        let (dx, dy) = ((k % w) as i32 - x, (k / w) as i32 - y);
        let near = dx.abs() <= 5 && (-1..=9).contains(&dy);
        if !near {
            assert_eq!(a, b, "cell ({}, {}) moved", k % w, k / w);
        }
    }
    assert_ne!(e.chars()[i], e.target_chars()[i]);
}

#[test]
fn ambient_bubbles_stay_inside_the_glass() {
    let mut e = engine(Effect::Fizz, 8);
    run_until_done(&mut e, 60.0);
    e.set_ambient(0.0);
    for _ in 0..60 {
        e.step(1.0 / 30.0);
    }
    assert_eq!(e.chars(), e.target_chars(), "no ambient bubbles at rate 0");
    e.set_ambient(20.0);
    let mut changed = 0;
    for _ in 0..90 {
        e.step(1.0 / 30.0);
        for (k, (a, b)) in e.chars().iter().zip(e.target_chars()).enumerate() {
            if a != b {
                changed += 1;
                assert!(matches!(e.target_colors()[k], GLASS | LIME | DARK | HIGH), "bubble outside the glass at {k}");
            }
        }
    }
    assert!(changed > 0, "no bubble rose");
    assert!(e.done(), "ambient bubbles do not reopen the effect");
}
