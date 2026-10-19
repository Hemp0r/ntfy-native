#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub fn popover_position(
    anchor: &AnchorRect,
    size_logical: (f32, f32),
    scale: f32,
    monitor_physical: (f64, f64, f64, f64),
) -> (f32, f32) {
    const MARGIN: f64 = 8.0;
    const GAP: f64 = 4.0;
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let (w, h) = (f64::from(size_logical.0), f64::from(size_logical.1));
    let (mx, my) = (monitor_physical.0 / scale, monitor_physical.1 / scale);
    let (mw, mh) = (monitor_physical.2 / scale, monitor_physical.3 / scale);
    let (ax, ay) = (anchor.x / scale, anchor.y / scale);
    let (aw, ah) = (anchor.w / scale, anchor.h / scale);

    let x = clamp(ax + aw / 2.0 - w, mx + MARGIN, mx + mw - w - MARGIN);
    let below = ay + ah / 2.0 < my + mh / 2.0;
    let y = if below { ay + ah + GAP } else { ay - h - GAP };
    let y = clamp(y, my + MARGIN, my + mh - h - MARGIN);
    (x as f32, y as f32)
}

fn clamp(v: f64, min: f64, max: f64) -> f64 {
    // A popover larger than the monitor sticks to the top/left edge.
    v.min(max).max(min)
}

pub fn parse_anchor(s: &str) -> Option<AnchorRect> {
    let mut it = s
        .split(',')
        .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()));
    let rect = AnchorRect {
        x: it.next()??,
        y: it.next()??,
        w: it.next()??,
        h: it.next()??,
    };
    it.next().is_none().then_some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (360.0, 480.0);

    fn anchor(x: f64, y: f64, w: f64, h: f64) -> AnchorRect {
        AnchorRect { x, y, w, h }
    }

    #[test]
    fn popover_position_below_top_tray() {
        let a = anchor(2400.0, 0.0, 48.0, 48.0);
        let (x, y) = popover_position(&a, SIZE, 2.0, (0.0, 0.0, 2880.0, 1800.0));
        assert_eq!(y, 24.0 + 4.0);
        assert_eq!(x, 1212.0 - 360.0);
    }

    #[test]
    fn popover_position_above_bottom_tray() {
        let a = anchor(1800.0, 1040.0, 32.0, 40.0);
        let (x, y) = popover_position(&a, SIZE, 1.0, (0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(y, 1040.0 - 480.0 - 4.0);
        assert_eq!(x, 1816.0 - 360.0);
    }

    #[test]
    fn popover_position_clamped_to_monitor() {
        let m = (0.0, 0.0, 1920.0, 1080.0);
        let (x, _) = popover_position(&anchor(1910.0, 1040.0, 20.0, 40.0), SIZE, 1.0, m);
        assert_eq!(x, 1920.0 - 360.0 - 8.0);
        let (x, _) = popover_position(&anchor(10.0, 0.0, 20.0, 24.0), SIZE, 1.0, m);
        assert_eq!(x, 8.0);
        // second monitor left of the primary
        let m2 = (-1920.0, 0.0, 1920.0, 1080.0);
        let (x, y) = popover_position(&anchor(-5.0, 0.0, 20.0, 24.0), SIZE, 1.0, m2);
        assert_eq!(x, -360.0 - 8.0);
        assert_eq!(y, 28.0);
        let (x, _) = popover_position(&anchor(-1910.0, 0.0, 20.0, 24.0), SIZE, 1.0, m2);
        assert_eq!(x, -1920.0 + 8.0);
    }

    #[test]
    fn popover_position_invalid_scale_falls_back_to_one() {
        let a = anchor(1800.0, 1040.0, 32.0, 40.0);
        let m = (0.0, 0.0, 1920.0, 1080.0);
        let expected = popover_position(&a, SIZE, 1.0, m);
        for scale in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(popover_position(&a, SIZE, scale, m), expected, "{scale}");
        }
    }

    #[test]
    fn anchor_parsing() {
        assert_eq!(
            parse_anchor("10,20.5,30,40"),
            Some(anchor(10.0, 20.5, 30.0, 40.0))
        );
        assert_eq!(
            parse_anchor("-10, 2,3,4"),
            Some(anchor(-10.0, 2.0, 3.0, 4.0))
        );
        assert_eq!(parse_anchor("1,2,3"), None);
        assert_eq!(parse_anchor("1,2,3,4,5"), None);
        assert_eq!(parse_anchor("a,2,3,4"), None);
        assert_eq!(parse_anchor("1,2,NaN,4"), None);
    }
}
