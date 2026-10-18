//! Procedural 32×32 tray icons.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Ok,
    Problem,
    Paused,
    NotConfigured,
}

const SIZE: u32 = 32;
/// Subsamples per axis for anti-aliased edges.
const SUB: u32 = 4;
const CENTER: f32 = 16.0;
const OUTER: f32 = 12.0;
const INNER: f32 = 8.5;
const DOT: (f32, f32) = (25.0, 25.0);
const DOT_RADIUS: f32 = 5.5;
/// Space cut out of the ring around the problem dot.
const DOT_GAP: f32 = 8.0;

fn dist(x: f32, y: f32, (cx, cy): (f32, f32)) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
}

fn inside(kind: IconKind, x: f32, y: f32) -> bool {
    let d = dist(x, y, (CENTER, CENTER));
    let ring = (INNER..=OUTER).contains(&d);
    match kind {
        IconKind::Ok => d <= OUTER,
        IconKind::NotConfigured => ring,
        IconKind::Problem => {
            let dot = dist(x, y, DOT);
            dot <= DOT_RADIUS || (ring && dot > DOT_GAP)
        }
        IconKind::Paused => {
            (7.0..25.0).contains(&y) && ((9.0..14.0).contains(&x) || (18.0..23.0).contains(&x))
        }
    }
}

/// 32×32 RGBA pixels. `template` yields black with alpha (macOS menu bar
/// template image); otherwise the state color.
pub fn rgba(kind: IconKind, template: bool) -> (Vec<u8>, u32, u32) {
    let (r, g, b) = match (template, kind) {
        (true, _) => (0, 0, 0),
        (false, IconKind::Ok) => (0x2A, 0x8A, 0x3A),
        (false, IconKind::Problem) => (0xC6, 0x28, 0x28),
        (false, IconKind::Paused | IconKind::NotConfigured) => (0x88, 0x88, 0x88),
    };
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut hits = 0;
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let fx = x as f32 + (sx as f32 + 0.5) / SUB as f32;
                    let fy = y as f32 + (sy as f32 + 0.5) / SUB as f32;
                    hits += u32::from(inside(kind, fx, fy));
                }
            }
            let alpha = (hits * 255 / (SUB * SUB)) as u8;
            px.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    (px, SIZE, SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(kind: IconKind, x: u32, y: u32) -> u8 {
        let (px, w, _) = rgba(kind, true);
        px[((y * w + x) * 4 + 3) as usize]
    }

    #[test]
    fn icon_dimensions() {
        for kind in [
            IconKind::Ok,
            IconKind::Problem,
            IconKind::Paused,
            IconKind::NotConfigured,
        ] {
            for template in [true, false] {
                let (px, w, h) = rgba(kind, template);
                assert_eq!((w, h, px.len()), (32, 32, 32 * 32 * 4));
            }
        }
    }

    #[test]
    fn ok_icon_center_opaque_problem_center_transparent() {
        assert_eq!(alpha_at(IconKind::Ok, 16, 16), 255);
        assert_eq!(alpha_at(IconKind::Problem, 16, 16), 0);
    }
}
