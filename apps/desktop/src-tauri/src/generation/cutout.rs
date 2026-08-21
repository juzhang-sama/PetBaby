use image::{DynamicImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum CutoutError {
    #[error("image too small: {0}x{1}")]
    TooSmall(u32, u32),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CandidateQualityStatus {
    Acceptable,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QualityReason {
    ExcessiveTransparency,
    InteriorHoles,
    LowContrastSubject,
    NonUniformBackground,
    InvalidSourceAlpha,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CutoutStrategy {
    SourceAlpha,
    ChromaKey,
    OpaqueFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateQualityReportV1 {
    pub schema_version: u32,
    pub status: CandidateQualityStatus,
    pub reasons: Vec<QualityReason>,
    pub opaque_ratio: f32,
    pub transparent_ratio: f32,
    pub partial_alpha_ratio: f32,
    pub visible_bounds: Option<[u32; 4]>,
}

impl CandidateQualityReportV1 {
    pub fn is_acceptable(&self) -> bool {
        self.status == CandidateQualityStatus::Acceptable
    }

    pub fn is_user_confirmable(&self) -> bool {
        self.status == CandidateQualityStatus::NeedsReview
            && self.visible_bounds.is_some()
            && self.transparent_ratio.is_finite()
            && self.transparent_ratio > 0.0
            && self.opaque_ratio.is_finite()
            && self.opaque_ratio < 1.0
            && !self.reasons.is_empty()
            && self.reasons.iter().all(|reason| {
                matches!(
                    reason,
                    QualityReason::ExcessiveTransparency | QualityReason::InteriorHoles
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CutoutResult {
    pub rgba: RgbaImage,
    pub report: CandidateQualityReportV1,
    pub strategy: CutoutStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceAlphaState {
    Absent,
    Meaningful,
    Invalid,
}

pub fn estimate_background(rgb: &[u8], width: u32, height: u32) -> [u8; 3] {
    let border = border_samples(rgb, width, height);
    median_color(&border)
}

pub fn is_uniform_background(rgb: &[u8], width: u32, height: u32, bg: [u8; 3], tol: u8) -> bool {
    let samples = border_samples(rgb, width, height);
    let within_expected_background = samples.iter().all(|p| {
        p[0].abs_diff(bg[0]) <= tol && p[1].abs_diff(bg[1]) <= tol && p[2].abs_diff(bg[2]) <= tol
    });
    let median = median_color(&samples);
    let mutually_consistent = samples
        .iter()
        .all(|sample| rgb_l1_distance(*sample, median) <= u32::from(tol));
    within_expected_background && mutually_consistent
}

fn rgb_l1_distance(left: [u8; 3], right: [u8; 3]) -> u32 {
    left[0].abs_diff(right[0]) as u32
        + left[1].abs_diff(right[1]) as u32
        + left[2].abs_diff(right[2]) as u32
}

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
            let dist = rgb_l1_distance([r, g, b], bg);
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

fn is_green_screen(rgb: &[u8], width: u32, height: u32) -> bool {
    let samples = border_samples(rgb, width, height);
    let strong_green = samples
        .iter()
        .filter(|sample| {
            let strongest_other = sample[0].max(sample[2]);
            sample[1] >= 120 && sample[1].saturating_sub(strongest_other) >= 100
        })
        .count();
    strong_green * 100 >= samples.len() * 95
}

fn is_green_screen_pixel(pixel: [u8; 3]) -> bool {
    let strongest_other = pixel[0].max(pixel[2]);
    pixel[1] >= 100 && pixel[1].saturating_sub(strongest_other) >= 35
}

fn green_screen_remove(rgb: &[u8], width: u32, height: u32) -> Result<RgbaImage, CutoutError> {
    if width < 16 || height < 16 {
        return Err(CutoutError::TooSmall(width, height));
    }

    let mut background = vec![false; (width * height) as usize];
    let mut queue = std::collections::VecDeque::new();
    let mut enqueue = |x: u32, y: u32| {
        let index = (y * width + x) as usize;
        let offset = index * 3;
        let pixel = [rgb[offset], rgb[offset + 1], rgb[offset + 2]];
        if !background[index] && is_green_screen_pixel(pixel) {
            background[index] = true;
            queue.push_back((x, y));
        }
    };
    for x in 0..width {
        enqueue(x, 0);
        enqueue(x, height - 1);
    }
    for y in 1..height - 1 {
        enqueue(0, y);
        enqueue(width - 1, y);
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
            let offset = index * 3;
            let pixel = [rgb[offset], rgb[offset + 1], rgb[offset + 2]];
            if !background[index] && is_green_screen_pixel(pixel) {
                background[index] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    let mut out = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            let offset = index * 3;
            let pixel = [rgb[offset], rgb[offset + 1], rgb[offset + 2]];
            let alpha = if background[index] {
                let dominance = pixel[1].saturating_sub(pixel[0].max(pixel[2]));
                (((100u16.saturating_sub(u16::from(dominance))) * 255) / 65).min(255) as u8
            } else {
                255
            };
            out.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], alpha]));
        }
    }
    Ok(out)
}

pub fn quality_report(rgba: &RgbaImage, strategy: CutoutStrategy) -> CandidateQualityReportV1 {
    let (width, height) = rgba.dimensions();
    let total = width * height;
    if total == 0 {
        let mut reasons = vec![QualityReason::ExcessiveTransparency];
        if strategy == CutoutStrategy::SourceAlpha {
            reasons.push(QualityReason::InvalidSourceAlpha);
        }
        if strategy == CutoutStrategy::OpaqueFallback {
            reasons.push(QualityReason::NonUniformBackground);
        }
        return CandidateQualityReportV1 {
            schema_version: 1,
            status: CandidateQualityStatus::NeedsReview,
            reasons,
            opaque_ratio: 0.0,
            transparent_ratio: 0.0,
            partial_alpha_ratio: 0.0,
            visible_bounds: None,
        };
    }

    let mut opaque = 0u32;
    let mut transparent = 0u32;
    let mut partial = 0u32;
    let mut visible_bounds: Option<[u32; 4]> = None;

    for (x, y, pixel) in rgba.enumerate_pixels() {
        match pixel[3] {
            0..=31 => transparent += 1,
            32..=223 => partial += 1,
            224..=255 => opaque += 1,
        }
        if pixel[3] >= 32 {
            visible_bounds = Some(match visible_bounds {
                Some([min_x, min_y, max_x, max_y]) => {
                    [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
                }
                None => [x, y, x, y],
            });
        }
    }

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
            if alpha < 32 && !reachable[(y * width + x) as usize] {
                interior_holes += 1;
            }
        }
    }

    let visible = opaque + partial;
    let mut reasons = Vec::new();
    if visible as f32 / total as f32 <= 0.03 {
        reasons.push(QualityReason::ExcessiveTransparency);
    }
    if visible > 0 && interior_holes as f32 / visible as f32 > 0.02 {
        reasons.push(QualityReason::InteriorHoles);
    }
    if has_low_contrast_subject(rgba) {
        reasons.push(QualityReason::LowContrastSubject);
    }
    if strategy == CutoutStrategy::OpaqueFallback {
        reasons.push(QualityReason::NonUniformBackground);
    }
    if strategy == CutoutStrategy::SourceAlpha
        && source_alpha_state(rgba) != SourceAlphaState::Meaningful
    {
        reasons.push(QualityReason::InvalidSourceAlpha);
    }

    let visible_bounds = visible_bounds
        .map(|[min_x, min_y, max_x, max_y]| [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1]);
    let status = if reasons.is_empty() {
        CandidateQualityStatus::Acceptable
    } else {
        CandidateQualityStatus::NeedsReview
    };
    CandidateQualityReportV1 {
        schema_version: 1,
        status,
        reasons,
        opaque_ratio: opaque as f32 / total as f32,
        transparent_ratio: transparent as f32 / total as f32,
        partial_alpha_ratio: partial as f32 / total as f32,
        visible_bounds,
    }
}

fn source_alpha_state(rgba: &RgbaImage) -> SourceAlphaState {
    let total = u64::from(rgba.width()) * u64::from(rgba.height());
    if total == 0 {
        return SourceAlphaState::Invalid;
    }

    let mut has_non_opaque = false;
    let mut visible = 0u64;
    let mut transparent = 0u64;
    let mut material_partial = false;
    for pixel in rgba.pixels() {
        has_non_opaque |= pixel[3] < 255;
        match pixel[3] {
            0..=31 => transparent += 1,
            32..=223 => {
                visible += 1;
                material_partial = true;
            }
            224..=255 => visible += 1,
        }
    }

    if !has_non_opaque {
        SourceAlphaState::Absent
    } else if visible > 0 && (material_partial || transparent * 100 >= total) {
        SourceAlphaState::Meaningful
    } else {
        SourceAlphaState::Invalid
    }
}

fn has_low_contrast_subject(rgba: &RgbaImage) -> bool {
    let mut visible_sum = [0u64; 3];
    let mut transparent_sum = [0u64; 3];
    let mut visible_count = 0u64;
    let mut transparent_count = 0u64;
    for pixel in rgba.pixels() {
        let (sum, count) = if pixel[3] >= 32 {
            (&mut visible_sum, &mut visible_count)
        } else {
            (&mut transparent_sum, &mut transparent_count)
        };
        for channel in 0..3 {
            sum[channel] += u64::from(pixel[channel]);
        }
        *count += 1;
    }
    if visible_count == 0 || transparent_count == 0 {
        return false;
    }

    let contrast: u64 = (0..3)
        .map(|channel| {
            (visible_sum[channel] / visible_count)
                .abs_diff(transparent_sum[channel] / transparent_count)
        })
        .sum();
    contrast <= 60
}

fn remove_opaque_background(rgb_img: &image::RgbImage) -> CutoutResult {
    let (width, height) = rgb_img.dimensions();
    let rgb = rgb_img.as_raw();
    let estimated_background = estimate_background(rgb, width, height);
    let green_screen = is_green_screen(rgb, width, height);
    let (rgba, strategy) =
        if green_screen || is_uniform_background(rgb, width, height, estimated_background, 40) {
            let cutout = if green_screen {
                green_screen_remove(rgb, width, height)
            } else {
                chroma_remove(rgb, width, height, 40)
            };
            let rgba = cutout.unwrap_or_else(|_| {
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
            (rgba, CutoutStrategy::ChromaKey)
        } else {
            // non-uniform background: keep opaque (degraded path, needs calibration)
            (
                DynamicImage::ImageRgb8(rgb_img.clone()).to_rgba8(),
                CutoutStrategy::OpaqueFallback,
            )
        };
    CutoutResult {
        report: quality_report(&rgba, strategy),
        rgba,
        strategy,
    }
}

pub fn remove_background(img: &DynamicImage) -> CutoutResult {
    let rgba = img.to_rgba8();
    match source_alpha_state(&rgba) {
        SourceAlphaState::Meaningful | SourceAlphaState::Invalid => CutoutResult {
            report: quality_report(&rgba, CutoutStrategy::SourceAlpha),
            rgba,
            strategy: CutoutStrategy::SourceAlpha,
        },
        SourceAlphaState::Absent => remove_opaque_background(&img.to_rgb8()),
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

    fn light_subject_fixture(bg: [u8; 3], subject: [u8; 3], outline: [u8; 3]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(200, 200, Rgba([bg[0], bg[1], bg[2], 255]));
        for y in 40..160 {
            for x in 40..160 {
                let color = if x < 48 || x >= 152 || y < 48 || y >= 152 {
                    outline
                } else {
                    subject
                };
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
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
        let report = quality_report(&rgba, CutoutStrategy::ChromaKey);
        assert_eq!(report.status, CandidateQualityStatus::Acceptable);

        // all-transparent image must be rejected
        let empty = RgbaImage::new(100, 100);
        let empty_report = quality_report(&empty, CutoutStrategy::SourceAlpha);
        assert_eq!(empty_report.status, CandidateQualityStatus::NeedsReview);
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
        let report = quality_report(&img, CutoutStrategy::ChromaKey);
        assert!(report.reasons.contains(&QualityReason::InteriorHoles));
        assert_eq!(report.status, CandidateQualityStatus::NeedsReview);
    }

    #[test]
    fn dark_subject_is_reported_acceptable() {
        let img = solid_bg_with_subject([226, 226, 226], [30, 30, 30]);
        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.strategy, CutoutStrategy::ChromaKey);
        assert_eq!(result.report.status, CandidateQualityStatus::Acceptable);
        assert!(result.report.reasons.is_empty());
        assert_eq!(result.report.visible_bounds, Some([60, 60, 80, 80]));
    }

    #[test]
    fn saturated_uniform_backdrop_is_removed() {
        let img = solid_bg_with_subject([0, 255, 0], [72, 94, 86]);

        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.strategy, CutoutStrategy::ChromaKey);
        assert_eq!(result.rgba.get_pixel(5, 5)[3], 0);
        assert_eq!(result.rgba.get_pixel(100, 100)[3], 255);
        assert_eq!(result.report.status, CandidateQualityStatus::Acceptable);
    }

    #[test]
    fn varied_green_screen_is_removed_without_erasing_an_enclosed_green_eye() {
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let variation = ((x + y) % 40) as u8;
                img.put_pixel(
                    x,
                    y,
                    Rgba([8 + variation, 250 - variation / 2, 6 + variation, 255]),
                );
            }
        }
        for y in 40..160 {
            for x in 40..160 {
                img.put_pixel(x, y, Rgba([72, 64, 68, 255]));
            }
        }
        for y in 85..115 {
            for x in 85..115 {
                img.put_pixel(x, y, Rgba([110, 175, 72, 255]));
            }
        }

        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.strategy, CutoutStrategy::ChromaKey);
        assert_eq!(result.rgba.get_pixel(5, 5)[3], 0);
        assert_eq!(result.rgba.get_pixel(50, 50)[3], 255);
        assert_eq!(result.rgba.get_pixel(100, 100)[3], 255);
        assert_eq!(result.report.status, CandidateQualityStatus::Acceptable);
    }

    #[test]
    fn light_subject_is_reported_needs_review() {
        let img = light_subject_fixture([226, 226, 226], [244, 230, 224], [30, 30, 30]);
        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.report.status, CandidateQualityStatus::NeedsReview);
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::InteriorHoles));
    }

    #[test]
    fn meaningful_source_alpha_is_preserved_byte_for_byte() {
        let mut img = RgbaImage::from_pixel(32, 32, Rgba([240, 240, 240, 255]));
        img.put_pixel(8, 8, Rgba([10, 20, 30, 77]));

        let result = remove_background(&DynamicImage::ImageRgba8(img.clone()));

        assert_eq!(result.strategy, CutoutStrategy::SourceAlpha);
        assert_eq!(result.rgba, img);
        assert!(!result
            .report
            .reasons
            .contains(&QualityReason::InvalidSourceAlpha));
    }

    #[test]
    fn fully_transparent_source_alpha_is_reported_invalid() {
        let img = RgbaImage::from_pixel(32, 32, Rgba([10, 20, 30, 0]));

        let result = remove_background(&DynamicImage::ImageRgba8(img.clone()));

        assert_eq!(result.strategy, CutoutStrategy::SourceAlpha);
        assert_eq!(result.rgba, img);
        assert_eq!(result.report.status, CandidateQualityStatus::NeedsReview);
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::InvalidSourceAlpha));
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::ExcessiveTransparency));
        assert_eq!(result.report.visible_bounds, None);
    }

    #[test]
    fn single_nearly_opaque_alpha_pixel_is_reported_invalid() {
        let mut img = RgbaImage::from_pixel(32, 32, Rgba([240, 240, 240, 255]));
        img.put_pixel(8, 8, Rgba([10, 20, 30, 254]));

        let result = remove_background(&DynamicImage::ImageRgba8(img.clone()));

        assert_eq!(result.strategy, CutoutStrategy::SourceAlpha);
        assert_eq!(result.rgba, img);
        assert_eq!(result.report.status, CandidateQualityStatus::NeedsReview);
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::InvalidSourceAlpha));
    }

    #[test]
    fn quality_report_calculates_alpha_buckets_and_visible_bounds() {
        let mut img = RgbaImage::new(6, 1);
        for (x, alpha) in [0, 31, 32, 223, 224, 255].into_iter().enumerate() {
            let channel = if x < 2 { 240 } else { 20 };
            img.put_pixel(x as u32, 0, Rgba([channel, channel, channel, alpha]));
        }

        let report = quality_report(&img, CutoutStrategy::SourceAlpha);

        assert!((report.transparent_ratio - (2.0 / 6.0)).abs() < f32::EPSILON);
        assert!((report.partial_alpha_ratio - (2.0 / 6.0)).abs() < f32::EPSILON);
        assert!((report.opaque_ratio - (2.0 / 6.0)).abs() < f32::EPSILON);
        assert_eq!(report.visible_bounds, Some([2, 0, 4, 1]));
    }

    #[test]
    fn quality_report_flags_a_low_contrast_subject() {
        let mut img = RgbaImage::from_pixel(32, 32, Rgba([226, 226, 226, 0]));
        for y in 8..24 {
            for x in 8..24 {
                img.put_pixel(x, y, Rgba([235, 230, 228, 255]));
            }
        }

        let report = quality_report(&img, CutoutStrategy::ChromaKey);

        assert!(report.reasons.contains(&QualityReason::LowContrastSubject));
        assert_eq!(report.status, CandidateQualityStatus::NeedsReview);
    }

    #[test]
    fn opaque_fallback_reports_non_uniform_background() {
        let img = RgbaImage::from_pixel(32, 32, Rgba([20, 40, 60, 255]));

        let report = quality_report(&img, CutoutStrategy::OpaqueFallback);

        assert!(report
            .reasons
            .contains(&QualityReason::NonUniformBackground));
        assert_eq!(report.status, CandidateQualityStatus::NeedsReview);
    }

    #[test]
    fn non_uniform_border_uses_opaque_fallback() {
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let channel = if x < 100 { 186 } else { 255 };
                img.put_pixel(x, y, Rgba([channel, channel, channel, 255]));
            }
        }

        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.strategy, CutoutStrategy::OpaqueFallback);
        assert_eq!(result.report.status, CandidateQualityStatus::NeedsReview);
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::NonUniformBackground));
    }

    #[test]
    fn l1_non_uniform_border_uses_opaque_fallback() {
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let channel = if x < 100 { 206 } else { 246 };
                img.put_pixel(x, y, Rgba([channel, channel, channel, 255]));
            }
        }

        let result = remove_background(&DynamicImage::ImageRgba8(img));

        assert_eq!(result.strategy, CutoutStrategy::OpaqueFallback);
        assert_eq!(result.report.status, CandidateQualityStatus::NeedsReview);
        assert!(result
            .report
            .reasons
            .contains(&QualityReason::NonUniformBackground));
    }
}
