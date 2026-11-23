//! A padlock drawn in code, used for the tray icon and the lock screen, so
//! there are no image files to ship.

/// Padlock pixels: `size` × `size` RGBA in the given colour, anti-aliased,
/// transparent background.
pub fn padlock_rgba(size: usize, rgb: [u8; 3]) -> Vec<u8> {
    const SS: usize = 4; // 4×4 samples per pixel
    let mut out = vec![0u8; size * size * 4];
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0usize;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if inside_padlock(x, y) {
                        hits += 1;
                    }
                }
            }
            let i = (py * size + px) * 4;
            out[i..i + 3].copy_from_slice(&rgb);
            out[i + 3] = (hits * 255 / (SS * SS)) as u8;
        }
    }
    out
}

/// Shape test in a 0..1 square.
fn inside_padlock(x: f32, y: f32) -> bool {
    // Body: rounded rectangle.
    let (l, r, t, b, rad) = (0.16, 0.84, 0.44, 0.94, 0.09);
    let in_body = {
        let cx = x.clamp(l + rad, r - rad);
        let cy = y.clamp(t + rad, b - rad);
        x >= l && x <= r && y >= t && y <= b && (x - cx).powi(2) + (y - cy).powi(2) <= rad * rad
    };
    // Keyhole: a circle and a slot cut out of the body.
    let keyhole = (x - 0.5).powi(2) + (y - 0.63).powi(2) <= 0.075f32.powi(2)
        || ((x - 0.5).abs() <= 0.032 && (0.63..=0.80).contains(&y));
    if in_body {
        return !keyhole;
    }
    // Shackle: the top half of a ring, plus two short legs down into the body.
    let (cx, cy, outer, inner) = (0.5, 0.33, 0.26, 0.15);
    let d2 = (x - cx).powi(2) + (y - cy).powi(2);
    let ring = d2 <= outer * outer && d2 >= inner * inner;
    (ring && y <= cy)
        || (y > cy
            && y < t + 0.02
            && ((x - (cx - outer)) >= 0.0 && x - (cx - outer) <= outer - inner
                || (x - (cx + inner)) >= 0.0 && x - (cx + inner) <= outer - inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_body_and_hole() {
        let px = padlock_rgba(64, [255, 255, 255]);
        let alpha = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0); // corner is empty
        assert_eq!(alpha(16, 50), 255); // body
        assert_eq!(alpha(32, 40), 0); // keyhole
        assert_eq!(alpha(32, 6), 255); // top of the shackle
        assert_eq!(alpha(32, 16), 0); // inside the shackle
    }
}
