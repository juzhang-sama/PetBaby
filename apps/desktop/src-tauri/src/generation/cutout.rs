use image::{DynamicImage, Rgba, RgbaImage};

#[derive(Debug, thiserror::Error)]
#[expect(dead_code)] // consumed by the pipeline in M4 Task 6
pub enum CutoutError {
    #[error("image too small: {0}x{1}")]
    TooSmall(u32, u32),
}

#[derive(Debug, Clone, Copy)]
#[expect(dead_code)] // consumed by the pipeline in M4 Task 6
pub struct QualityReport {
    pub opaque_ratio: f32,
    pub transparent_ratio: f32,
    pub interior_holes: bool,
}

#[expect(dead_code)]
impl QualityReport {
    pub fn is_acceptable(&self) -> bool {
        // gate against over-cutout: too much transparency overall, or holes inside
        // the subject area (light fur wrongly removed)
        self.opaque_ratio > 0.03 && self.opaque_ratio < 0.95 && !self.interior_holes
    }
}

#[expect(dead_code)] // internal helper
pub fn estimate_background(rgb: &[u8], width: u32, height: u32) -> [u8; 3] {
    let border = border_samples(rgb, width, height);
    median_color(&border)
}

#[expect(dead_code)]
pub fn is_uniform_background(rgb: &[u8], width: u32, height: u32, bg: [u8; 3], tol: u8) -> bool {
    let samples = border_samples(rgb, width, height);
    samples.iter().all(|p| {
        p[0].abs_diff(bg[0]) <= tol && p[1].abs_diff(bg[1]) <= tol && p[2].abs_diff(bg[2]) <= tol
    })
}

#[expect(dead_code)]
fn border_samples(rgb: &[u8], width: u32, height: u32) -> Vec<[u8; 3]> {
    let depth = (height / 20).max(1);
    let mut samples = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let on_border = y < depth || y >= height - depth || x < depth || x >= width - depth;
            if on_border {
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

#[expect(dead_code)]
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
                let t = ((dist - tolerance as u32) as f32 / 60.0).clamp(0.0, 1.0);
                (t * 255.0) as u8
            };
            out.put_pixel(x, y, Rgba([r, g, b, alpha]));
        }
    }
    Ok(out)
}

#[expect(dead_code)]
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

#[expect(dead_code)] // consumed by the pipeline in M4 Task 6
pub fn remove_background(img: &DynamicImage) -> (RgbaImage, QualityReport) {
    let rgb_img = img.to_rgb8();
    let (width, height) = rgb_img.dimensions();
    let rgb = rgb_img.as_raw();
    let (rgba, report) = if is_uniform_background(rgb, width, height, [226, 226, 226], 40)
        || is_uniform_background(rgb, width, height, [255, 255, 255], 40)
    {
        let rgba = chroma_remove(rgb, width, height, 40).unwrap_or_else(|_| {
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
        let report = quality_report(&rgba);
        (rgba, report)
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
}
