use image::{DynamicImage, Rgba, RgbaImage};
use std::collections::VecDeque;

/// Chroma-key distance threshold. Kept low on purpose: light fur on the
/// light-gray generation background is close in color, and a large tolerance
/// erases it (over-cutout).
const CHROMA_TOLERANCE: u8 = 12;
const CHROMA_FEATHER: u32 = 6;

#[derive(Debug, thiserror::Error)]
pub enum CutoutError {
    #[error("image too small: {0}x{1}")]
    TooSmall(u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub struct QualityReport {
    pub opaque_ratio: f32,
    pub transparent_ratio: f32,
    pub interior_holes: bool,
}

impl QualityReport {
    pub fn is_acceptable(&self) -> bool {
        // gate against over-cutout: too much transparency overall, or holes inside
        // the subject area (light fur wrongly removed)
        self.opaque_ratio > 0.03
            && self.opaque_ratio < 0.95
            && self.transparent_ratio < 0.97
            && !self.interior_holes
    }
}

pub fn estimate_background(rgb: &[u8], width: u32, height: u32) -> [u8; 3] {
    let samples = background_samples(rgb, width, height);
    median_color(&samples)
}

pub fn is_uniform_background(rgb: &[u8], width: u32, height: u32, bg: [u8; 3], tol: u8) -> bool {
    let samples = background_samples(rgb, width, height);
    if samples.is_empty() {
        return false;
    }
    // tolerate a small fraction of border outliers (watermark/logo/edge marks)
    let mut outliers = 0usize;
    for p in &samples {
        if p[0].abs_diff(bg[0]) > tol || p[1].abs_diff(bg[1]) > tol || p[2].abs_diff(bg[2]) > tol {
            outliers += 1;
        }
    }
    outliers as f32 / samples.len() as f32 <= 0.01
}

/// Background samples taken from the four corners. Generated pets are often
/// composed close to the top/bottom edges, so full border strips can include
/// pet pixels and wrongly disqualify a uniform background; pets rarely occupy
/// all four corners.
fn background_samples(rgb: &[u8], width: u32, height: u32) -> Vec<[u8; 3]> {
    let size = (((width.min(height) as f32) * 0.08) as u32).max(8);
    let corners = [
        (0u32, 0u32),
        (width.saturating_sub(size), 0u32),
        (0u32, height.saturating_sub(size)),
        (width.saturating_sub(size), height.saturating_sub(size)),
    ];
    let mut samples = Vec::new();
    for (x0, y0) in corners {
        let x1 = (x0 + size).min(width);
        let y1 = (y0 + size).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * width + x) * 3) as usize;
                samples.push([rgb[i], rgb[i + 1], rgb[i + 2]]);
            }
        }
    }
    samples
}

fn median_color(samples: &[[u8; 3]]) -> [u8; 3] {
    let mut reds: Vec<u8> = samples.iter().map(|p| p[0]).collect();
    let mut greens: Vec<u8> = samples.iter().map(|p| p[1]).collect();
    let mut blues: Vec<u8> = samples.iter().map(|p| p[2]).collect();
    reds.sort_unstable();
    greens.sort_unstable();
    blues.sort_unstable();
    let mid = |v: &Vec<u8>| v[v.len() / 2];
    [mid(&reds), mid(&greens), mid(&blues)]
}

pub fn chroma_remove(
    rgb: &[u8],
    width: u32,
    height: u32,
    tolerance: u8,
) -> Result<RgbaImage, CutoutError> {
    if width < 16 || height < 16 {
        return Err(CutoutError::TooSmall(width, height));
    }
    let bg = estimate_background(rgb, width, height);
    let mut out = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 3) as usize;
            let r = rgb[i];
            let g = rgb[i + 1];
            let b = rgb[i + 2];
            let dist =
                r.abs_diff(bg[0]) as u32 + g.abs_diff(bg[1]) as u32 + b.abs_diff(bg[2]) as u32;
            let alpha = if dist <= tolerance as u32 {
                0
            } else {
                let t = ((dist - tolerance as u32) as f32 / CHROMA_FEATHER as f32).clamp(0.0, 1.0);
                (t * 255.0) as u8
            };
            out.put_pixel(x, y, Rgba([r, g, b, alpha]));
        }
    }
    Ok(out)
}

/// Restore transparent pixels that are sealed inside the opaque subject.
/// These are over-cutout artifacts (fur removed by chroma keying), so they are
/// filled back to opaque while outside-connected background stays transparent.
pub fn fill_interior_holes(rgba: &mut RgbaImage) {
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return;
    }
    let mut reachable = vec![false; (width * height) as usize];
    let mut queue = VecDeque::new();
    let transparent = |pixel: &Rgba<u8>| pixel[3] < 32;

    for x in 0..width {
        for &y in &[0u32, height - 1] {
            let index = (y * width + x) as usize;
            if transparent(rgba.get_pixel(x, y)) && !reachable[index] {
                reachable[index] = true;
                queue.push_back((x, y));
            }
        }
    }
    for y in 0..height {
        for &x in &[0u32, width - 1] {
            let index = (y * width + x) as usize;
            if transparent(rgba.get_pixel(x, y)) && !reachable[index] {
                reachable[index] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let index = (ny * width + nx) as usize;
            if !reachable[index] && transparent(rgba.get_pixel(nx, ny)) {
                reachable[index] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            if !reachable[index] && transparent(rgba.get_pixel(x, y)) {
                let mut pixel = *rgba.get_pixel(x, y);
                pixel[3] = 255;
                rgba.put_pixel(x, y, pixel);
            }
        }
    }
}

/// Drop every opaque pixel that is not part of the largest connected component.
/// With a low chroma tolerance, background noise can remain as small opaque
/// islands; the pet is the largest component, so those islands are removed.
pub fn keep_largest_opaque_component(rgba: &mut RgbaImage) {
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return;
    }
    let total = (width * height) as usize;
    let mut component = vec![0u32; total];
    let mut sizes: Vec<u32> = Vec::new();
    let mut queue = VecDeque::new();
    let mut next_id = 1u32;

    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            if rgba.get_pixel(x, y)[3] < 32 || component[index] != 0 {
                continue;
            }
            let id = next_id;
            next_id += 1;
            component[index] = id;
            queue.push_back((x, y));
            let mut size = 0u32;
            while let Some((cx, cy)) = queue.pop_front() {
                size += 1;
                for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    let nx = cx as i64 + dx;
                    let ny = cy as i64 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                        continue;
                    }
                    let (nx, ny) = (nx as u32, ny as u32);
                    let nindex = (ny * width + nx) as usize;
                    if component[nindex] == 0 && rgba.get_pixel(nx, ny)[3] >= 32 {
                        component[nindex] = id;
                        queue.push_back((nx, ny));
                    }
                }
            }
            sizes.push(size);
        }
    }

    let Some(&largest) = sizes.iter().max() else {
        return;
    };
    let largest_id = sizes.iter().position(|size| *size == largest).unwrap() as u32 + 1;
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            if component[index] != 0 && component[index] != largest_id {
                let mut pixel = *rgba.get_pixel(x, y);
                pixel[3] = 0;
                rgba.put_pixel(x, y, pixel);
            }
        }
    }
}

/// Binary subject mask based on chroma distance alone: a pixel is subject only
/// when its distance clearly exceeds the background (tolerance + feather).
/// Blended contour pixels (distance 13..=18) are treated as background so the
/// color-feather gray fringe disappears from the final edge.
fn core_subject_mask(
    rgb: &[u8],
    width: u32,
    height: u32,
    bg: [u8; 3],
    tolerance: u8,
    feather: u32,
) -> Vec<bool> {
    let threshold = tolerance as u32 + feather;
    let total = (width * height) as usize;
    let mut mask = vec![false; total];
    for (i, px) in rgb.chunks_exact(3).enumerate() {
        let dist = px[0].abs_diff(bg[0]) as u32
            + px[1].abs_diff(bg[1]) as u32
            + px[2].abs_diff(bg[2]) as u32;
        mask[i] = dist > threshold;
    }
    mask
}

/// Final subject mask for edge refinement: chroma distance above the feather
/// ceiling, intersected with the component-filtered alpha (so background specks
/// removed by `keep_largest_opaque_component` stay removed).
fn subject_mask(
    rgb: &[u8],
    width: u32,
    height: u32,
    bg: [u8; 3],
    tolerance: u8,
    feather: u32,
    rgba: &RgbaImage,
) -> Vec<bool> {
    let chroma = core_subject_mask(rgb, width, height, bg, tolerance, feather);
    let alpha: Vec<bool> = rgba.as_raw().chunks_exact(4).map(|p| p[3] >= 32).collect();
    chroma
        .iter()
        .zip(alpha.iter())
        .map(|(c, a)| *c && *a)
        .collect()
}

/// 3x3 majority filter: removes single-pixel notches and islands from the
/// chroma mask so the final binary edge follows a clean contour instead of
/// amplifying per-pixel noise.
fn smooth_mask(mask: &[bool], width: u32, height: u32) -> Vec<bool> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let mut count = 0usize;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx >= 0
                        && ny >= 0
                        && nx < w as i64
                        && ny < h as i64
                        && mask[ny as usize * w + nx as usize]
                    {
                        count += 1;
                    }
                }
            }
            out[y * w + x] = count >= 5;
        }
    }
    out
}

/// Replace the color-feather alpha with a clean binary edge.
///
/// The chroma feather makes blended contour pixels semi-transparent, which
/// reads as a gray fringe around the pet. The provided mask is the clean
/// subject/core mask (chroma distance above the feather ceiling, intersected
/// with the component-filtered alpha). After a 3x3 majority smooth, alpha is
/// binary: fully opaque inside, fully transparent outside. Thin features
/// (whiskers, fur strands) stay opaque instead of becoming a semi-transparent
/// haze, which a distance-transform anti-alias would cause.
pub fn refine_edge(rgba: &mut RgbaImage, mask: &[bool]) {
    let (width, height) = rgba.dimensions();
    let total = (width * height) as usize;
    if total == 0 || mask.len() != total {
        return;
    }
    let mask = smooth_mask(mask, width, height);
    let raw_mut = rgba.as_mut();
    for (i, alpha) in raw_mut.chunks_exact_mut(4).enumerate() {
        alpha[3] = if mask[i] { 255 } else { 0 };
    }
}

/// Crop the cutout to the opaque subject bounds with a small padding, so the
/// pet fills the window instead of floating in large transparent margins.
pub fn crop_to_subject(rgba: &RgbaImage) -> RgbaImage {
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return rgba.clone();
    }
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if rgba.get_pixel(x, y)[3] >= 32 {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !found {
        return rgba.clone();
    }
    let pad = ((width.min(height) as f32 * 0.03) as u32).max(6);
    let x0 = min_x.saturating_sub(pad);
    let y0 = min_y.saturating_sub(pad);
    let x1 = (max_x + pad).min(width - 1);
    let y1 = (max_y + pad).min(height - 1);
    let mut out = RgbaImage::new(x1 - x0 + 1, y1 - y0 + 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.put_pixel(x - x0, y - y0, *rgba.get_pixel(x, y));
        }
    }
    out
}

pub fn quality_report(rgba: &RgbaImage) -> QualityReport {
    let (width, height) = rgba.dimensions();
    let total = width * height;
    let mut opaque = 0u32;
    let mut transparent = 0u32;

    // flood fill from the border: transparent pixels reachable from the outside
    // are background; the rest are interior holes (over-cutout)
    let mut reachable = vec![false; (width * height) as usize];
    let mut queue = std::collections::VecDeque::new();
    for x in 0..width {
        for &y in &[0u32, height - 1] {
            if rgba.get_pixel(x, y)[3] < 32 && !reachable[(y * width + x) as usize] {
                reachable[(y * width + x) as usize] = true;
                queue.push_back((x, y));
            }
        }
    }
    for y in 0..height {
        for &x in &[0u32, width - 1] {
            if rgba.get_pixel(x, y)[3] < 32 && !reachable[(y * width + x) as usize] {
                reachable[(y * width + x) as usize] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let index = (ny * width + nx) as usize;
            if !reachable[index] && rgba.get_pixel(nx, ny)[3] < 32 {
                reachable[index] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    let mut interior_holes = 0u32;
    for y in 0..height {
        for x in 0..width {
            let alpha = rgba.get_pixel(x, y)[3];
            if alpha >= 32 {
                opaque += 1;
            } else {
                transparent += 1;
                if !reachable[(y * width + x) as usize] {
                    interior_holes += 1;
                }
            }
        }
    }
    QualityReport {
        opaque_ratio: opaque as f32 / total as f32,
        transparent_ratio: transparent as f32 / total as f32,
        interior_holes: opaque > 0 && interior_holes as f32 / opaque as f32 > 0.02,
    }
}

pub fn remove_background(img: &DynamicImage) -> (RgbaImage, QualityReport) {
    let rgb_img = img.to_rgb8();
    let (width, height) = rgb_img.dimensions();
    let rgb = rgb_img.as_raw();
    // adapt to whatever uniform background the provider produced (white per the
    // generation prompt, light-gray from older prompts, or any other uniform
    // color); the outlier tolerance keeps small border watermarks from
    // disqualifying an otherwise uniform background
    let bg = estimate_background(rgb, width, height);
    let (rgba, report) = if is_uniform_background(rgb, width, height, bg, 40) {
        let mut rgba = chroma_remove(rgb, width, height, CHROMA_TOLERANCE).unwrap_or_else(|_| {
            // fallback: opaque image
            let mut out = RgbaImage::new(width, height);
            for y in 0..height {
                for x in 0..width {
                    let i = ((y * width + x) * 3) as usize;
                    out.put_pixel(x, y, Rgba([rgb[i], rgb[i + 1], rgb[i + 2], 255]));
                }
            }
            out
        });
        fill_interior_holes(&mut rgba);
        keep_largest_opaque_component(&mut rgba);
        let report = quality_report(&rgba);
        let mask = subject_mask(
            rgb,
            width,
            height,
            bg,
            CHROMA_TOLERANCE,
            CHROMA_FEATHER,
            &rgba,
        );
        refine_edge(&mut rgba, &mask);
        (crop_to_subject(&rgba), report)
    } else {
        // non-uniform background: keep opaque (degraded path, needs calibration)
        let mut out = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 3) as usize;
                out.put_pixel(x, y, Rgba([rgb[i], rgb[i + 1], rgb[i + 2], 255]));
            }
        }
        let report = quality_report(&out);
        (out, report)
    };
    (rgba, report)
}

/// Cutout with an enforced quality gate: if the result is not acceptable
/// (e.g. subject is indistinguishable from the background), return the opaque
/// raw image so the pipeline degrades instead of shipping an over-cutout.
pub fn remove_background_guarded(img: &DynamicImage) -> DynamicImage {
    let (rgba, report) = remove_background(img);
    if report.is_acceptable() {
        DynamicImage::ImageRgba8(rgba)
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn solid_bg_with_subject(bg: [u8; 3], subject: [u8; 3]) -> RgbaImage {
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (60..140).contains(&x) && (60..140).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([subject[0], subject[1], subject[2], 255]));
                } else {
                    img.put_pixel(x, y, Rgba([bg[0], bg[1], bg[2], 255]));
                }
            }
        }
        img
    }

    #[test]
    fn estimates_background_from_borders() {
        let img = solid_bg_with_subject([226, 226, 226], [72, 94, 86]);
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let bg = estimate_background(&rgb, 200, 200);
        assert!(bg[0].abs_diff(226) <= 3);
        assert!(bg[1].abs_diff(226) <= 3);
    }

    #[test]
    fn chroma_remove_makes_background_transparent() {
        let img = solid_bg_with_subject([226, 226, 226], [72, 94, 86]);
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let rgba = chroma_remove(&rgb, 200, 200, 40).unwrap();
        assert_eq!(rgba.get_pixel(5, 5)[3], 0);
        assert_eq!(rgba.get_pixel(100, 100)[3], 255);
    }

    #[test]
    fn quality_gate_rejects_empty_and_edge_holes() {
        let img = solid_bg_with_subject([226, 226, 226], [72, 94, 86]);
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let rgba = chroma_remove(&rgb, 200, 200, 40).unwrap();
        let report = quality_report(&rgba);
        assert!(report.is_acceptable());

        // all-transparent image must be rejected
        let empty = RgbaImage::new(100, 100);
        let empty_report = quality_report(&empty);
        assert!(!empty_report.is_acceptable());
    }

    #[test]
    fn over_cutout_detected_as_interior_holes() {
        // solid subject with a sealed transparent hole inside (light fur removed)
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (40..160).contains(&x) && (40..160).contains(&y);
                let in_hole = (90..110).contains(&x) && (90..110).contains(&y);
                let alpha = if !in_subject || in_hole { 0 } else { 255 };
                img.put_pixel(x, y, Rgba([100, 100, 100, alpha]));
            }
        }
        let report = quality_report(&img);
        assert!(report.interior_holes);
        assert!(!report.is_acceptable());
    }

    #[test]
    fn light_fur_on_light_gray_is_not_eaten() {
        // regression: tolerance 40 erased light fur close to the light-gray
        // generation background, producing over-cutout
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (40..160).contains(&x) && (40..160).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([242, 238, 230, 255])); // light fur
                } else {
                    img.put_pixel(x, y, Rgba([226, 226, 226, 255])); // light-gray bg
                }
            }
        }
        let (rgba, report) = remove_background(&DynamicImage::ImageRgba8(img));
        assert_eq!(
            rgba.get_pixel(100, 100)[3],
            255,
            "light fur must survive the cutout"
        );
        assert!(
            report.is_acceptable(),
            "gate must accept the preserved subject"
        );
        assert!(report.opaque_ratio > 0.2);
    }

    #[test]
    fn interior_holes_are_filled() {
        // a sealed transparent region inside the subject (fur removed by chroma)
        // must be restored to opaque, while outside background stays transparent
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (40..160).contains(&x) && (40..160).contains(&y);
                let in_hole = (90..110).contains(&x) && (90..110).contains(&y);
                if in_subject && !in_hole {
                    img.put_pixel(x, y, Rgba([60, 80, 70, 255]));
                } else if in_hole {
                    img.put_pixel(x, y, Rgba([242, 238, 230, 0]));
                }
            }
        }
        fill_interior_holes(&mut img);
        assert_eq!(
            img.get_pixel(100, 100)[3],
            255,
            "sealed hole must be restored"
        );
        assert_eq!(
            img.get_pixel(5, 5)[3],
            0,
            "outside background stays transparent"
        );
    }

    #[test]
    fn remove_background_guarded_falls_back_to_opaque_when_cutout_unacceptable() {
        // uniform light image: subject is indistinguishable from background, so
        // the quality gate must degrade to the opaque raw image (no alpha)
        let img = RgbaImage::from_pixel(200, 200, Rgba([226, 226, 226, 255]));
        let result = remove_background_guarded(&DynamicImage::ImageRgba8(img));
        assert!(
            !result.color().has_alpha(),
            "unacceptable cutout must degrade to opaque raw"
        );
    }

    #[test]
    fn near_background_fur_stays_fully_opaque() {
        // regression: fur that is close to the background color must still be
        // fully opaque; a wide tolerance + feather left it semi-transparent
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (40..160).contains(&x) && (40..160).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([238, 234, 228, 255])); // fur, dist=22 from bg
                } else {
                    img.put_pixel(x, y, Rgba([226, 226, 226, 255]));
                }
            }
        }
        let (rgba, _) = remove_background(&DynamicImage::ImageRgba8(img));
        assert_eq!(
            rgba.get_pixel(100, 100)[3],
            255,
            "near-background fur must be opaque"
        );
    }

    #[test]
    fn background_specks_are_removed() {
        // with a low chroma tolerance, background noise can stay opaque as
        // small islands; only the largest component (the pet) must survive
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (60..140).contains(&x) && (60..140).contains(&y);
                let in_speck = (8..28).contains(&x) && (8..28).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([60, 80, 70, 255]));
                } else if in_speck {
                    img.put_pixel(x, y, Rgba([235, 232, 228, 255])); // background noise
                }
            }
        }
        keep_largest_opaque_component(&mut img);
        assert_eq!(img.get_pixel(100, 100)[3], 255, "subject must survive");
        assert_eq!(
            img.get_pixel(16, 16)[3],
            0,
            "background speck must be dropped"
        );
        assert_eq!(img.get_pixel(5, 5)[3], 0, "background stays transparent");
    }

    #[test]
    fn edge_is_binary_and_clean() {
        // the final edge must be binary: no semi-transparent gray fringe at all
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let inside = (60..140).contains(&x) && (60..140).contains(&y);
                let alpha = if inside { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([100, 100, 100, alpha]));
            }
        }
        let mask: Vec<bool> = img.as_raw().chunks_exact(4).map(|p| p[3] >= 32).collect();
        refine_edge(&mut img, &mask);
        assert_eq!(img.get_pixel(100, 100)[3], 255, "deep inside stays opaque");
        assert_eq!(img.get_pixel(5, 5)[3], 0, "far outside stays transparent");
        let semi = img.pixels().filter(|p| p[3] > 0 && p[3] < 255).count();
        assert_eq!(semi, 0, "no gray fringe allowed");
    }

    #[test]
    fn thin_features_stay_opaque() {
        // whiskers/fur strands are 1-2px wide; they must stay fully opaque
        // instead of becoming a semi-transparent haze
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (40..160).contains(&x) && (40..160).contains(&y);
                // 2px-wide vertical whisker attached to the subject
                let in_whisker = (80..82).contains(&x) && (160..190).contains(&y);
                let alpha = if in_subject || in_whisker { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([120, 120, 120, alpha]));
            }
        }
        let mask: Vec<bool> = img.as_raw().chunks_exact(4).map(|p| p[3] >= 32).collect();
        refine_edge(&mut img, &mask);
        assert_eq!(
            img.get_pixel(80, 175)[3],
            255,
            "whisker must stay fully opaque"
        );
        assert_eq!(
            img.get_pixel(81, 175)[3],
            255,
            "whisker must stay fully opaque"
        );
    }

    #[test]
    fn blended_contour_ring_is_not_a_gray_fringe() {
        // subject fur is close to the background; a blended ring around the
        // contour (distance 16) used to become a semi-transparent gray fringe
        // under the color feather. The core mask (distance > tolerance +
        // feather = 18) must classify the ring as background.
        let bg = [226u8, 226, 226];
        let fur = [238u8, 234, 228]; // distance 22 from bg
        let ring = [235u8, 232, 227]; // distance 16 from bg
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let inside = (60..140).contains(&x) && (60..140).contains(&y);
                let on_ring = (57..60).contains(&x)
                    || (140..143).contains(&x)
                    || (57..60).contains(&y)
                    || (140..143).contains(&y);
                let color = if inside {
                    fur
                } else if on_ring {
                    ring
                } else {
                    bg
                };
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
            }
        }
        let rgb = DynamicImage::ImageRgba8(img).to_rgb8();
        let bg_est = estimate_background(&rgb, 200, 200);
        let mut rgba = chroma_remove(&rgb, 200, 200, CHROMA_TOLERANCE).unwrap();
        fill_interior_holes(&mut rgba);
        keep_largest_opaque_component(&mut rgba);
        let mask = subject_mask(
            &rgb,
            200,
            200,
            bg_est,
            CHROMA_TOLERANCE,
            CHROMA_FEATHER,
            &rgba,
        );
        refine_edge(&mut rgba, &mask);
        assert_eq!(
            rgba.get_pixel(58, 100)[3],
            0,
            "blended ring must be background, not a gray fringe"
        );
        assert!(
            rgba.get_pixel(61, 100)[3] > 180,
            "fur 1px inside the contour stays nearly opaque"
        );
        assert_eq!(
            rgba.get_pixel(100, 100)[3],
            255,
            "fur core stays fully opaque"
        );
    }

    #[test]
    fn uniform_background_with_border_anomaly_is_accepted() {
        // a small watermark/logo on the border must not disqualify a uniform
        // background; otherwise the pet degrades to an opaque gray rectangle
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (60..140).contains(&x) && (60..140).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([60, 80, 70, 255]));
                } else {
                    img.put_pixel(x, y, Rgba([226, 226, 226, 255]));
                }
            }
        }
        // dark mark on the right border, about 0.5% of all border pixels
        for y in 20..30 {
            for x in 196..200 {
                img.put_pixel(x, y, Rgba([60, 60, 60, 255]));
            }
        }
        let rgb = DynamicImage::ImageRgba8(img).to_rgb8();
        assert!(
            is_uniform_background(&rgb, 200, 200, [226, 226, 226], 40),
            "small border anomaly must be tolerated"
        );
    }

    #[test]
    fn full_frame_subject_does_not_disqualify_uniform_background() {
        // guided-generated pets are composed close to the top/bottom edges;
        // border-strip sampling counted pet pixels as background outliers and
        // degraded the whole image to opaque. Corner sampling must still see
        // a uniform white background.
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (60..140).contains(&x) && (8..192).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([250, 150, 50, 255]));
                } else {
                    img.put_pixel(x, y, Rgba([254, 254, 254, 255]));
                }
            }
        }
        let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        assert!(
            is_uniform_background(&rgb, 200, 200, [254, 254, 254], 40),
            "pet near edges must not disqualify the background"
        );
        let (rgba, report) = remove_background(&DynamicImage::ImageRgba8(img));
        assert!(report.is_acceptable());
        assert_eq!(
            rgba.get_pixel(rgba.width() / 2, rgba.height() / 2)[3],
            255,
            "subject stays opaque"
        );
        assert_eq!(
            rgba.get_pixel(5, 5)[3],
            0,
            "corner padding stays transparent"
        );
    }

    #[test]
    fn any_uniform_background_is_cut() {
        // a non-light-gray uniform background (e.g. beige) must be accepted by
        // the adaptive check and cut, instead of degrading to opaque
        let img = solid_bg_with_subject([180, 170, 150], [60, 80, 70]);
        let (rgba, report) = remove_background(&DynamicImage::ImageRgba8(img));
        assert!(report.is_acceptable());
        assert_eq!(rgba.get_pixel(5, 5)[3], 0, "crop padding stays transparent");
        assert_eq!(
            rgba.get_pixel(rgba.width() / 2, rgba.height() / 2)[3],
            255,
            "subject stays opaque"
        );
    }

    #[test]
    #[ignore = "manual diagnostic: requires RAW_PNG and CUTOUT_OUT env vars"]
    fn debug_real_cutout() {
        use image::GenericImageView;
        let raw_path = std::env::var("RAW_PNG").expect("RAW_PNG env var");
        let out_path = std::env::var("CUTOUT_OUT").expect("CUTOUT_OUT env var");
        let img = image::open(&raw_path).expect("open raw png");
        let result = remove_background_guarded(&img);
        let (width, height) = result.dimensions();
        let mut semi = 0u32;
        let mut opaque = 0u32;
        match &result {
            image::DynamicImage::ImageRgba8(rgba) => {
                for p in rgba.pixels() {
                    if p[3] > 0 && p[3] < 255 {
                        semi += 1;
                    }
                    if p[3] >= 128 {
                        opaque += 1;
                    }
                }
            }
            _ => {
                println!("degraded to opaque raw (gate rejected)");
            }
        }
        result.save(&out_path).expect("save cutout");
        println!(
            "saved {out_path}: {width}x{height} semi={semi} opaque={opaque} ratio={:.3}",
            opaque as f32 / (width * height) as f32
        );
    }

    #[test]
    fn cutout_crops_to_subject_bounds() {
        // the generated canvas has large transparent margins; the cutout must
        // crop to the subject so the pet fills the window
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let in_subject = (60..140).contains(&x) && (60..140).contains(&y);
                if in_subject {
                    img.put_pixel(x, y, Rgba([72, 94, 86, 255]));
                } else {
                    img.put_pixel(x, y, Rgba([226, 226, 226, 255]));
                }
            }
        }
        let (rgba, _) = remove_background(&DynamicImage::ImageRgba8(img));
        let (w, h) = rgba.dimensions();
        assert!(w < 200 && h < 200, "subject must be cropped to its bounds");
        assert_eq!(
            rgba.get_pixel(0, 0)[3],
            0,
            "outside crop area stays transparent"
        );
        assert_eq!(rgba.get_pixel(w / 2, h / 2)[3], 255, "subject stays opaque");
    }
}
