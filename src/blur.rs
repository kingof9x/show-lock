//! Turns a screenshot into a small, heavily blurred background image.
//!
//! The image is shrunk first (so the blur is cheap) and the GPU stretches it
//! back up with smooth filtering, which softens it further.

/// An RGBA image, 4 bytes per pixel, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Longest side of the shrunken image, in pixels.
const TARGET_SIZE: usize = 320;

/// Shrink a BGRA screenshot (as Windows gives it) to about [`TARGET_SIZE`]
/// pixels on its longest side and blur it.
pub fn blurred_background(bgra: &[u8], width: usize, height: usize) -> Image {
    let factor = (width.max(height) / TARGET_SIZE).max(1);
    let mut img = downscale_bgra(bgra, width, height, factor);
    box_blur(&mut img, 4, 3);
    img
}

/// Average each `factor` × `factor` block into one pixel, converting BGRA to
/// RGBA with full opacity.
pub fn downscale_bgra(bgra: &[u8], width: usize, height: usize, factor: usize) -> Image {
    assert!(bgra.len() >= width * height * 4, "image buffer too small");
    let factor = factor.max(1);
    let (w, h) = ((width / factor).max(1), (height / factor).max(1));
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for sy in (y * factor)..((y + 1) * factor).min(height) {
                for sx in (x * factor)..((x + 1) * factor).min(width) {
                    let i = (sy * width + sx) * 4;
                    sum[0] += u32::from(bgra[i + 2]);
                    sum[1] += u32::from(bgra[i + 1]);
                    sum[2] += u32::from(bgra[i]);
                    n += 1;
                }
            }
            let o = (y * w + x) * 4;
            let n = n.max(1);
            rgba[o] = (sum[0] / n) as u8;
            rgba[o + 1] = (sum[1] / n) as u8;
            rgba[o + 2] = (sum[2] / n) as u8;
            rgba[o + 3] = 255;
        }
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

/// Repeated box blur, which approximates a Gaussian blur.
pub fn box_blur(img: &mut Image, radius: usize, passes: usize) {
    if radius == 0 || img.width == 0 || img.height == 0 {
        return;
    }
    let mut tmp = vec![0u8; img.rgba.len()];
    for _ in 0..passes {
        blur_pass(&img.rgba, &mut tmp, img.width, img.height, radius, true);
        blur_pass(&tmp, &mut img.rgba, img.width, img.height, radius, false);
    }
}

/// One horizontal or vertical box-blur pass with a running sum; edge pixels
/// are repeated past the border.
fn blur_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, pos: usize| {
        if horizontal {
            (line * w + pos) * 4
        } else {
            (pos * w + line) * 4
        }
    };
    let window = (2 * r + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 4];
        for k in 0..=2 * r {
            let pos = k.saturating_sub(r).min(len - 1);
            let i = idx(line, pos);
            for c in 0..4 {
                sum[c] += u32::from(src[i + c]);
            }
        }
        for pos in 0..len {
            let o = idx(line, pos);
            for c in 0..4 {
                dst[o + c] = (sum[c] / window) as u8;
            }
            let out = idx(line, pos.saturating_sub(r));
            let inn = idx(line, (pos + r + 1).min(len - 1));
            for c in 0..4 {
                sum[c] = sum[c] + u32::from(src[inn + c]) - u32::from(src[out + c]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn downscale_converts_and_averages() {
        let img = downscale_bgra(&solid(8, 4, [10, 20, 30, 0]), 8, 4, 2);
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(&img.rgba[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn blur_keeps_solid_colour() {
        let mut img = downscale_bgra(&solid(16, 9, [50, 100, 150, 255]), 16, 9, 1);
        let before = img.clone();
        box_blur(&mut img, 3, 3);
        assert_eq!(img, before);
    }

    #[test]
    fn blur_spreads_a_bright_pixel() {
        let mut bgra = solid(9, 9, [0, 0, 0, 255]);
        let centre = (4 * 9 + 4) * 4;
        bgra[centre..centre + 3].copy_from_slice(&[255, 255, 255]);
        let mut img = downscale_bgra(&bgra, 9, 9, 1);
        box_blur(&mut img, 1, 1);
        let at = |x: usize, y: usize| img.rgba[(y * 9 + x) * 4];
        assert!(at(4, 4) < 255 && at(4, 4) > 0);
        assert!(at(3, 4) > 0 && at(4, 3) > 0);
        assert_eq!(at(0, 0), 0);
    }

    #[test]
    fn background_is_small() {
        let img = blurred_background(&solid(1920, 1080, [1, 2, 3, 255]), 1920, 1080);
        assert!(img.width <= TARGET_SIZE * 2 && img.width >= TARGET_SIZE / 2);
        assert_eq!(img.rgba.len(), img.width * img.height * 4);
    }
}
