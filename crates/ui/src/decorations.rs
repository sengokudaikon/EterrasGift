//! Eterra's decorations behind the `Eterra` global: how far her tree has bloomed, the
//! band's frame, and the ash. Each frame is a pure function of the clock, so the band and the ash agree and a capture is
//! reproducible. Frames are asked for by the view (`EterraFrames`) at a low rate while something moves, and never with
//! Windows animation effects off: then the bloom settles at once and the ash holds a still frame.

use crate::{Eterra, Mote, Tokens};
use eg_core::bloom::BloomPhase;
use slint::{ComponentHandle, Global, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The carved eye in the keyframes (image pixels): the reveal spreads from it and the band keeps it on the page's axis.
const EYE: (f64, f64) = (783.0, 120.0);

thread_local! {
    static FROZEN: Cell<Option<Instant>> = const { Cell::new(None) };
    static ART: RefCell<Option<Rc<Art>>> = const { RefCell::new(None) };
}

/// Holds the decorations' clock at `now` (None lets it run), for captures that show a frame partway through.
pub fn freeze_clock(now: Option<Instant>) {
    FROZEN.set(now);
}

fn now() -> Instant {
    FROZEN.get().unwrap_or_else(Instant::now)
}

/// How far Eterra's tree has bloomed, from 0 (wilted, all ash) to 1 (in flower, the ash gone). Each phase sets a target;
/// the level eases there from wherever it is, slowly when blooming and quickly when withering, so it never runs ahead of
/// the real state.
#[derive(Clone, Debug)]
pub struct BloomClock {
    levels: [f64; 4],
    rise: f64,
    fall: f64,
    from: f64,
    to: f64,
    changed: Instant,
}

impl BloomClock {
    /// `levels` per phase in phase order; `rise` and `fall` in seconds.
    pub fn new(levels: [f64; 4], rise: f64, fall: f64, now: Instant) -> BloomClock {
        BloomClock {
            levels,
            rise,
            fall,
            from: levels[0],
            to: levels[0],
            changed: now,
        }
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    pub fn levels(&self) -> &[f64; 4] {
        &self.levels
    }

    /// Heads for the phase's level from `now`; without animation the level is there at once.
    pub fn head(&mut self, phase: BloomPhase, animate: bool, now: Instant) {
        let target = self.levels[phase as usize];
        self.from = if animate { self.level(now) } else { target };
        self.to = target;
        self.changed = now;
    }

    /// Jumps to the target, for still frames.
    pub fn settle(&mut self) {
        self.from = self.to;
    }

    #[allow(clippy::float_cmp, reason = "exact: level returns the target itself once the move is over")]
    pub fn is_settled(&self, now: Instant) -> bool {
        self.level(now) == self.to
    }

    pub fn level(&self, now: Instant) -> f64 {
        let seconds = if self.to >= self.from { self.rise } else { self.fall };
        let t = if seconds <= 0.0 {
            1.0
        } else {
            (now.saturating_duration_since(self.changed).as_secs_f64() / seconds).clamp(0.0, 1.0)
        };
        if t >= 1.0 { self.to } else { self.from + (self.to - self.from) * smoothstep(t) }
    }
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The two keyframes either side of the bloom `level`, and how far between them it is (0 to 1).
pub fn segment(levels: &[f64; 4], level: f64) -> (usize, usize, f64) {
    for k in 0..levels.len() - 1 {
        if level < levels[k + 1] {
            let span = levels[k + 1] - levels[k];
            let reveal = if span <= 0.0 { 1.0 } else { ((level - levels[k]) / span).clamp(0.0, 1.0) };
            return (k, k + 1, reveal);
        }
    }
    (levels.len() - 1, levels.len() - 1, 0.0)
}

/// The ash field's design values (Tokens).
#[derive(Clone, Copy, Debug)]
pub struct AshLook {
    pub limit: usize,
    pub ember_share: f64,
    pub ash_opacity: f64,
    pub ember_opacity: f64,
    pub glow_scale: f64,
    pub glow_opacity: f64,
    pub still_level: f64,
    /// How far into its drift a field starts, so the first frame (and every capture) is already full (seconds).
    pub warm_up: f64,
    /// Each mote's constants: how long one rise takes (seconds), how much of the height it climbs, how far (pixels) and
    /// how slowly (seconds) it sways, and its radius (pixels), embers and ash apart.
    pub rise: Spread,
    pub climb: Spread,
    pub sway: Spread,
    pub sway_period: Spread,
    pub ember_radius: Spread,
    pub ash_radius: Spread,
    /// How much of the bloom range a mote takes to fade out once its threshold is passed.
    pub clear_band: f64,
}

/// A mote's constant: the base plus its own share of the spread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    pub base: f64,
    pub range: f64,
}

impl Spread {
    fn at(self, share: f64) -> f64 {
        self.base + self.range * share
    }
}

/// The ash at `time` seconds into its drift, thinned for the bloom `level`; `still` is the cold dusting (no embers).
/// The field draws as many of the motes as its area allows, by `index`.
pub fn motes(time: f64, level: f64, still: bool, look: &AshLook) -> Vec<Mote> {
    let mut motes = Vec::new();
    for i in 0..look.limit as u32 {
        // Per-mote constants: how long one rise takes, how far it climbs, how it sways and how large it is.
        let period = look.rise.at(noise(i, 1));
        let phase = time / period + noise(i, 2);
        let cycle = phase.floor();
        let progress = phase - cycle;
        let ember = noise(i, 3) < look.ember_share;
        if ember && still {
            continue;
        }
        // The bloom clears the ash mote by mote: each goes once the level passes its own threshold.
        let band = look.clear_band;
        let keep = ((noise(i, 10) * (1.0 - band) + band - level) / band).clamp(0.0, 1.0);
        if keep <= 0.0 {
            continue;
        }
        let rise = look.climb.at(noise(i, 4));
        let sway = look.sway.at(noise(i, 5)) * (std::f64::consts::TAU * (time / look.sway_period.at(noise(i, 6)) + noise(i, 7))).sin();
        // Each rise starts from a fresh spot, so the field never settles into fixed columns.
        let x = noise(i, (100 + cycle as i64) as u32);
        let y = 1.0 - rise * progress;
        let radius = if ember { look.ember_radius.at(noise(i, 8)) } else { look.ash_radius.at(noise(i, 9)) };
        let opacity = keep * if ember { look.ember_opacity } else { look.ash_opacity } * (std::f64::consts::PI * progress).sin().powf(1.5);
        let mote = |radius: f64, opacity: f64| Mote {
            index: i as i32,
            x: x as f32,
            y: y as f32,
            sway: sway as f32,
            radius: radius as f32,
            opacity: opacity as f32,
            ember,
        };
        if ember {
            motes.push(mote(radius * look.glow_scale, opacity * look.glow_opacity));
        }
        motes.push(mote(radius, opacity));
    }
    motes
}

fn spread(base: f32, range: f32) -> Spread {
    Spread {
        base: f64::from(base),
        range: f64::from(range),
    }
}

/// A stable value in [0, 1) for a mote and channel, so every frame agrees on each mote's constants.
fn noise(mote: u32, channel: u32) -> f64 {
    let mut hash = mote.wrapping_mul(0x9E37_79B1) ^ channel.wrapping_mul(0x85EB_CA77);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^= hash >> 15;
    f64::from(hash) / 4_294_967_296.0
}

/// The keyframes' pixels, and how far their twigs hang: what the reveal and the content margin need.
pub struct Art {
    keyframes: Vec<SharedPixelBuffer<Rgba8Pixel>>,
    /// `reach[c]`: the lowest painted row (alpha > 64) in any keyframe within c columns either side of the eye.
    pub reach: Vec<i32>,
    pub deepest: i32,
}

impl Art {
    /// None when a keyframe cannot be decoded or they differ in size.
    pub fn new(keyframes: &[Image]) -> Option<Art> {
        let keyframes = keyframes.iter().map(Image::to_rgba8).collect::<Option<Vec<_>>>()?;
        let (width, height) = (keyframes.first()?.width(), keyframes[0].height());
        if keyframes.iter().any(|frame| frame.width() != width || frame.height() != height) {
            return None;
        }
        let mut hang = vec![0; width as usize];
        for frame in &keyframes {
            for (y, row) in frame.as_slice().chunks_exact(width as usize).enumerate() {
                for (x, pixel) in row.iter().enumerate() {
                    if pixel.a > 64 {
                        hang[x] = hang[x].max(y as i32);
                    }
                }
            }
        }
        let eye = EYE.0 as usize;
        let reach = (0..=eye.max(hang.len() - eye))
            .map(|c| hang[eye.saturating_sub(c)..(eye + c).min(hang.len())].iter().copied().max().unwrap_or(0))
            .collect::<Vec<_>>();
        let deepest = hang.iter().copied().max().unwrap_or(0);
        Some(Art { keyframes, reach, deepest })
    }

    /// Keyframe `key` as the band shows it: its rows below the cut, its ends dissolving into the ground.
    pub fn keyframe(&self, key: usize, cut: &Cut) -> SharedPixelBuffer<Rgba8Pixel> {
        self.masked(key, cut, None)
    }

    /// Keyframe `key` spread out from the eye in a soft-edged circle that reaches the far corners at `reveal` 1.
    pub fn spread(&self, key: usize, reveal: f64, cut: &Cut) -> SharedPixelBuffer<Rgba8Pixel> {
        self.masked(key, cut, Some(reveal))
    }

    fn masked(&self, key: usize, cut: &Cut, reveal: Option<f64>) -> SharedPixelBuffer<Rgba8Pixel> {
        let source = &self.keyframes[key];
        let (width, height) = (source.width() as usize, source.height() as usize);
        let top = cut.rows.min(height);
        let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, (height - top) as u32);
        // Measured on screen, so divided by the band's scale into image pixels.
        let scale = cut.scale.max(f64::EPSILON);
        let side = (cut.side / scale).min(width as f64 / 2.0).max(f64::EPSILON);
        let ends = (0..width)
            .map(|x| ((x as f64 + 0.5) / side).min((width as f64 - x as f64 - 0.5) / side).clamp(0.0, 1.0))
            .collect::<Vec<_>>();
        let feather = cut.feather / scale;
        let far = EYE.0.max(width as f64 - EYE.0).max(1.0) + (height - top) as f64;
        let radius = reveal.map_or(f64::INFINITY, |reveal| (reveal * (far + feather)).max(1.0));
        let inner = (radius - feather).max(0.0);
        let (pixels, target) = (source.as_slice(), out.make_mut_slice());
        for (j, row) in target.chunks_exact_mut(width).enumerate() {
            let dy = (top + j) as f64 + 0.5 - EYE.1;
            if dy.abs() >= radius {
                continue;
            }
            let half = (radius * radius - dy * dy).sqrt();
            let first = (EYE.0 - half).floor().max(0.0) as usize;
            let last = ((EYE.0 + half).ceil().min(width as f64)) as usize;
            let from = &pixels[(top + j) * width..(top + j + 1) * width];
            for x in first..last {
                let dx = x as f64 + 0.5 - EYE.0;
                let distance = (dx * dx + dy * dy).sqrt();
                let circle = if distance <= inner {
                    1.0
                } else {
                    ((radius - distance) / (radius - inner)).clamp(0.0, 1.0)
                };
                let pixel = from[x];
                row[x] = Rgba8Pixel {
                    a: (f64::from(pixel.a) * circle * ends[x]).round() as u8,
                    ..pixel
                };
            }
        }
        out
    }
}

/// Where the band cuts and fades its keyframes. `rows` above the window's edge are image pixels; the rest is on screen:
/// the band's `scale`, the reveal's soft edge (`feather`) and how far its ends dissolve (`side`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub rows: usize,
    pub scale: f64,
    pub feather: f64,
    pub side: f64,
}

/// What a surface shows of the ash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ash {
    Hidden,
    /// Waiting for something real: drifting, thinned by the bloom.
    Drifting,
}

struct State {
    clock: BloomClock,
    phase: BloomPhase,
    ash: Ash,
    look: AshLook,
    /// When the frames began to run, for the ash's drift; None while they do not.
    started: Option<Instant>,
    /// The keyframe behind as last cut, kept while neither changes.
    behind: Option<(usize, Cut, Image)>,
}

/// The decorations of one window: its `Eterra` global, following the phase its surface maps its state onto.
pub struct Decorations<C: ComponentHandle + 'static> {
    window: Weak<C>,
    art: Option<Rc<Art>>,
    state: RefCell<State>,
    /// The drifting motes, one model for the window's life: each frame replaces its rows rather than handing the view a
    /// new model, which would rebuild every mote element 24 times a second.
    motes: Rc<VecModel<Mote>>,
}

impl<C> Decorations<C>
where
    C: ComponentHandle + 'static,
    for<'a> Eterra<'a>: Global<'a, C>,
    for<'a> Tokens<'a>: Global<'a, C>,
{
    /// Reads the keyframes and the design values, and answers the window's frame requests.
    pub fn install(window: &C) -> Rc<Decorations<C>> {
        let eterra = window.global::<Eterra>();
        let tokens = window.global::<Tokens>();
        let art = ART.with_borrow_mut(|art| {
            if art.is_none() {
                let keyframes = eterra.get_keyframes().iter().collect::<Vec<_>>();
                *art = Art::new(&keyframes).map(Rc::new);
            }
            art.clone()
        });
        if let Some(art) = &art {
            eterra.set_reach(ModelRc::new(VecModel::from(art.reach.clone())));
            eterra.set_deepest(art.deepest);
        }
        let look = AshLook {
            limit: tokens.get_ash_mote_limit().max(0) as usize,
            ember_share: f64::from(tokens.get_ash_ember_share()),
            ash_opacity: f64::from(tokens.get_ash_opacity()),
            ember_opacity: f64::from(tokens.get_ember_opacity()),
            glow_scale: f64::from(tokens.get_ember_glow_scale()),
            glow_opacity: f64::from(tokens.get_ember_glow_opacity()),
            still_level: f64::from(tokens.get_ash_still_level()),
            warm_up: f64::from(tokens.get_ash_warm_up_seconds()),
            rise: spread(tokens.get_ash_rise_seconds(), tokens.get_ash_rise_seconds_spread()),
            climb: spread(tokens.get_ash_climb(), tokens.get_ash_climb_spread()),
            sway: spread(tokens.get_ash_sway(), tokens.get_ash_sway_spread()),
            sway_period: spread(tokens.get_ash_sway_seconds(), tokens.get_ash_sway_seconds_spread()),
            ember_radius: spread(tokens.get_ember_radius(), tokens.get_ember_radius_spread()),
            ash_radius: spread(tokens.get_ash_radius(), tokens.get_ash_radius_spread()),
            clear_band: f64::from(tokens.get_ash_clear_band()),
        };
        eterra.set_still_motes(ModelRc::new(VecModel::from(motes(look.warm_up, look.still_level, true, &look))));
        let levels = [
            tokens.get_bloom_level_wilted(),
            tokens.get_bloom_level_budding(),
            tokens.get_bloom_level_opening(),
            tokens.get_bloom_level_bloom(),
        ]
        .map(f64::from);
        let clock = BloomClock::new(levels, f64::from(tokens.get_bloom_in_seconds()), f64::from(tokens.get_bloom_out_seconds()), now());
        let decorations = Rc::new(Decorations {
            window: window.as_weak(),
            art,
            state: RefCell::new(State {
                clock,
                phase: BloomPhase::Wilted,
                ash: Ash::Hidden,
                look,
                started: None,
                behind: None,
            }),
            motes: Rc::new(VecModel::default()),
        });
        eterra.set_motes(ModelRc::from(decorations.motes.clone()));
        eterra.on_frame({
            let decorations = Rc::downgrade(&decorations);
            move || {
                if let Some(decorations) = decorations.upgrade() {
                    decorations.draw();
                }
            }
        });
        decorations.draw();
        decorations
    }

    /// The surface came into view at `phase`: the tree is there at once, as it is when a page is opened.
    pub fn enter(&self, phase: BloomPhase, ash: Ash) {
        {
            let mut state = self.state.borrow_mut();
            state.phase = phase;
            state.ash = ash;
            state.clock.head(phase, false, now());
        }
        self.draw();
    }

    /// The surface moved to `phase`: the tree blooms or withers there, at once with animation effects off.
    pub fn follow(&self, phase: BloomPhase, ash: Ash) {
        {
            let mut state = self.state.borrow_mut();
            if state.phase != phase {
                state.phase = phase;
                let animate = self.window.upgrade().is_some_and(|window| window.global::<Tokens>().get_motion());
                state.clock.head(phase, animate, now());
            }
            state.ash = ash;
        }
        self.draw();
    }

    fn draw(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let eterra = window.global::<Eterra>();
        let tokens = window.global::<Tokens>();
        let motion = tokens.get_motion();
        let now = now();
        let mut state = self.state.borrow_mut();
        if !motion {
            state.clock.settle();
        }
        let level = state.clock.level(now);
        let settled = state.clock.is_settled(now);
        // Once the bloom has cleared the ash there is nothing left to drift.
        let drifting = state.ash == Ash::Drifting && !(state.clock.target() >= 1.0 && settled);
        let moving = !settled || drifting;
        if motion && moving {
            state.started.get_or_insert(now);
        } else {
            state.started = None;
        }

        let (behind, ahead, reveal) = segment(state.clock.levels(), level);
        if let Some(art) = &self.art {
            let cut = Cut {
                rows: tokens.get_eterra_band_cut().max(0.0) as usize,
                scale: f64::from(eterra.get_band_scale()),
                feather: f64::from(tokens.get_eterra_band_feather()),
                side: f64::from(tokens.get_eterra_band_side_feather()),
            };
            // The keyframe behind changes only between segments and with the band's size.
            if state.behind.as_ref().is_none_or(|(key, at, _)| (*key, *at) != (behind, cut)) {
                state.behind = Some((behind, cut, Image::from_rgba8(art.keyframe(behind, &cut))));
            }
            if let Some((_, _, image)) = &state.behind {
                eterra.set_behind(image.clone());
            }
            let spread = if reveal > 0.0 && ahead != behind {
                Image::from_rgba8(art.spread(ahead, reveal, &cut))
            } else {
                Image::default()
            };
            eterra.set_spread(spread);
        }
        // The earlier keyframe holds until the later one has nearly spread across, then fades out under it.
        let handover = f64::from(tokens.get_eterra_band_handover());
        eterra.set_behind_opacity((1.0 - smoothstep((reveal - handover) / (1.0 - handover))) as f32);

        let ash = match state.ash {
            Ash::Hidden => Vec::new(),
            Ash::Drifting => {
                let time = state.look.warm_up + state.started.map_or(Duration::ZERO, |started| now - started).as_secs_f64();
                motes(time, level, false, &state.look)
            }
        };
        // The same motes move: change their rows in place; only a change in their number resets the model.
        if self.motes.row_count() == ash.len() {
            for (row, mote) in ash.into_iter().enumerate() {
                self.motes.set_row_data(row, mote);
            }
        } else {
            self.motes.set_vec(ash);
        }
        eterra.set_moving(moving);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [f64; 4] = [0.0, 0.3, 0.6, 1.0];

    fn look() -> AshLook {
        AshLook {
            limit: 64,
            ember_share: 0.22,
            ash_opacity: 0.42,
            ember_opacity: 0.75,
            glow_scale: 3.5,
            glow_opacity: 0.18,
            still_level: 0.4,
            warm_up: 40.0,
            rise: Spread { base: 9.0, range: 9.0 },
            climb: Spread { base: 0.35, range: 0.4 },
            sway: Spread { base: 4.0, range: 10.0 },
            sway_period: Spread { base: 5.0, range: 4.0 },
            ember_radius: Spread { base: 1.0, range: 0.8 },
            ash_radius: Spread { base: 0.7, range: 1.2 },
            clear_band: 0.12,
        }
    }

    #[test]
    fn the_clock_eases_up_slowly_and_down_quickly() {
        let start = Instant::now();
        let mut clock = BloomClock::new(LEVELS, 2.4, 0.8, start);
        clock.head(BloomPhase::Bloom, true, start);
        assert_eq!(clock.level(start), 0.0);
        assert!((clock.level(start + Duration::from_millis(1200)) - 0.5).abs() < 1e-9, "smoothstep is halfway at half time");
        assert!(!clock.is_settled(start + Duration::from_millis(2399)));
        assert!(clock.is_settled(start + Duration::from_millis(2400)));

        let later = start + Duration::from_secs(3);
        clock.head(BloomPhase::Wilted, true, later);
        assert_eq!(clock.level(later), 1.0);
        assert!(clock.is_settled(later + Duration::from_millis(800)));
        assert_eq!(clock.level(later + Duration::from_millis(800)), 0.0);
    }

    #[test]
    fn a_new_phase_heads_on_from_wherever_the_level_is() {
        let start = Instant::now();
        let mut clock = BloomClock::new(LEVELS, 2.4, 0.8, start);
        clock.head(BloomPhase::Bloom, true, start);
        let midway = start + Duration::from_millis(1200);
        clock.head(BloomPhase::Opening, true, midway);
        assert!((clock.level(midway) - 0.5).abs() < 1e-9);
        // 0.5 → 0.6 is a rise, so it takes the rising time.
        assert!(!clock.is_settled(midway + Duration::from_millis(2000)));
        assert_eq!(clock.level(midway + Duration::from_millis(2400)), 0.6);
    }

    #[test]
    fn without_animation_the_level_is_there_at_once() {
        let start = Instant::now();
        let mut clock = BloomClock::new(LEVELS, 2.4, 0.8, start);
        clock.head(BloomPhase::Opening, false, start);
        assert_eq!(clock.level(start), 0.6);
        clock.head(BloomPhase::Bloom, true, start);
        clock.settle();
        assert!(clock.is_settled(start));
    }

    #[test]
    fn segments_name_the_keyframes_either_side() {
        assert_eq!(segment(&LEVELS, 0.0), (0, 1, 0.0));
        assert_eq!(segment(&LEVELS, 0.3), (1, 2, 0.0));
        let (behind, ahead, reveal) = segment(&LEVELS, 0.45);
        assert_eq!((behind, ahead), (1, 2));
        assert!((reveal - 0.5).abs() < 1e-9);
        assert_eq!(segment(&LEVELS, 1.0), (3, 3, 0.0));
    }

    #[test]
    fn the_bloom_clears_the_ash_and_still_ash_has_no_embers() {
        let look = look();
        let wilted = motes(look.warm_up, 0.0, false, &look);
        let opening = motes(look.warm_up, 0.6, false, &look);
        assert!(wilted.iter().any(|mote| mote.ember));
        assert!(opening.len() < wilted.len());
        assert!(motes(look.warm_up, 1.0, false, &look).is_empty());
        let still = motes(look.warm_up, look.still_level, true, &look);
        assert!(!still.is_empty());
        assert!(still.iter().all(|mote| !mote.ember));
    }

    #[test]
    fn motes_are_a_pure_function_of_time() {
        let look = look();
        assert_eq!(motes(52.5, 0.3, false, &look), motes(52.5, 0.3, false, &look));
        assert_ne!(motes(52.5, 0.3, false, &look), motes(53.0, 0.3, false, &look));
        assert!(
            motes(52.5, 0.0, false, &look)
                .iter()
                .all(|mote| (0.0..1.0).contains(&mote.x) && mote.y <= 1.0 && mote.opacity >= 0.0)
        );
    }

    fn keyframes() -> Vec<Image> {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/images");
        ["band-1-wilted.png", "band-2-budding.png", "band-3-opening.png", "band-4-bloom.png"]
            .iter()
            .map(|name| Image::load_from_path(&folder.join(name)).expect("a keyframe"))
            .collect()
    }

    #[test]
    fn the_reach_grows_with_the_column_and_ends_at_the_deepest_twig() {
        let art = Art::new(&keyframes()).expect("the keyframes decode");
        assert!(art.reach.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(*art.reach.last().unwrap(), art.deepest);
        assert!(
            art.deepest > 80 && art.deepest < 384,
            "the twigs hang below the cut and above the image's foot: {}",
            art.deepest
        );
    }

    #[test]
    fn the_spread_opens_from_the_eye_and_the_ends_dissolve() {
        let art = Art::new(&keyframes()).expect("the keyframes decode");
        let cut = Cut {
            rows: 80,
            scale: 0.5,
            feather: 140.0,
            side: 160.0,
        };
        let small = art.spread(3, 0.1, &cut);
        let full = art.spread(3, 1.0, &cut);
        let opaque = |frame: &SharedPixelBuffer<Rgba8Pixel>| frame.as_slice().iter().filter(|pixel| pixel.a > 0).count();
        assert!(opaque(&small) > 0);
        assert!(opaque(&small) < opaque(&full));
        // Nothing reaches the corners early on; fully spread, it is the keyframe as the band shows it.
        assert_eq!(small.as_slice()[0].a, 0);
        assert_eq!(full.as_slice(), art.keyframe(3, &cut).as_slice());
        // Between the dissolving ends, the band is the painting itself below the cut.
        let (source, shown) = (art.keyframes[3].as_slice(), art.keyframe(3, &cut));
        let row = 40;
        assert_eq!(
            shown.as_slice()[row * 1536 + 400..row * 1536 + 1100],
            source[(80 + row) * 1536 + 400..(80 + row) * 1536 + 1100]
        );
        assert!((0..1536).all(|x| shown.as_slice()[row * 1536 + x].a <= source[(80 + row) * 1536 + x].a));
        assert_eq!(shown.as_slice()[row * 1536].a, 0, "the very end is gone: {:?}", source[(80 + row) * 1536]);
    }
}
