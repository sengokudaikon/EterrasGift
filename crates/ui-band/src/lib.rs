//! Eterra's band as the window shows it: the keyframes cut below the window's edge, their ends dissolving into the ground,
//! and the next keyframe spreading out from her eye. Slint has no opacity masks, so the band is composited here, pixel by
//! pixel, into a `SharedPixelBuffer` the view shows as an `Image` (eg-ui's `decorations` asks for each frame).
//!
//! A crate of its own so that it is optimised in every profile: a frame of the spread is the window's heaviest work while
//! the bloom moves, and unoptimised it took most of a frame's time (the root `Cargo.toml` has the numbers).

#![forbid(unsafe_code)]

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// The carved eye in the keyframes (image pixels): the reveal spreads from it and the band keeps it on the page's axis.
const EYE: (f64, f64) = (783.0, 120.0);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn keyframes() -> Vec<Image> {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui-view/ui/images");
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
