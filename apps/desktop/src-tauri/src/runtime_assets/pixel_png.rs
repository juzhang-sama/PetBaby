use crate::creation::photo_avatar::domain::PixelAlphaReportV1;
use image::{ColorType, ImageFormat};
use std::collections::HashSet;

pub const MAX_PIXEL_PNG_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PixelPngInspection {
    pub width: u32,
    pub height: u32,
    pub visible_color_count: u16,
    pub alpha_report: PixelAlphaReportV1,
}

pub fn inspect_rgba_png(png: &[u8]) -> Result<PixelPngInspection, String> {
    if png.len() > MAX_PIXEL_PNG_BYTES {
        return Err("pixel avatar PNG exceeds 20 MiB".into());
    }
    let decoded = image::load_from_memory_with_format(png, ImageFormat::Png)
        .map_err(|error| format!("invalid pixel avatar PNG: {error}"))?;
    if decoded.color() != ColorType::Rgba8 {
        return Err("pixel avatar PNG must be RGBA8".into());
    }
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if !(1024..=2048).contains(&width)
        || !(1024..=2048).contains(&height)
        || u64::from(width) * u64::from(height) > 4_194_304
    {
        return Err("pixel avatar dimensions are outside 1024..2048".into());
    }
    let alpha: Vec<u8> = rgba.pixels().map(|pixel| pixel[3]).collect();
    let mut visible_pixels = 0_u32;
    let mut partial_alpha_pixels = 0_u32;
    let mut visible_colors = HashSet::new();
    let mut bounds_left = width;
    let mut bounds_top = height;
    let mut bounds_right = 0_u32;
    let mut bounds_bottom = 0_u32;
    let width_usize = usize::try_from(width).map_err(|_| "pixel avatar width is invalid")?;
    for (index, pixel) in rgba.pixels().enumerate() {
        let value = pixel[3];
        if value == 0 {
            continue;
        }
        visible_colors.insert([pixel[0], pixel[1], pixel[2]]);
        let x = u32::try_from(index % width_usize)
            .map_err(|_| "pixel avatar x coordinate is invalid")?;
        let y = u32::try_from(index / width_usize)
            .map_err(|_| "pixel avatar y coordinate is invalid")?;
        visible_pixels += 1;
        partial_alpha_pixels += u32::from(value < 255);
        bounds_left = bounds_left.min(x);
        bounds_top = bounds_top.min(y);
        bounds_right = bounds_right.max(x + 1);
        bounds_bottom = bounds_bottom.max(y + 1);
    }
    if visible_pixels == 0 {
        return Err("pixel avatar PNG is fully transparent".into());
    }
    let minimum_x_margin = (width * 2 + 99) / 100;
    let minimum_y_margin = (height * 2 + 99) / 100;
    let margins = [
        bounds_left,
        bounds_top,
        width - bounds_right,
        height - bounds_bottom,
    ];
    if margins[0] < minimum_x_margin
        || margins[2] < minimum_x_margin
        || margins[1] < minimum_y_margin
        || margins[3] < minimum_y_margin
    {
        return Err("pixel avatar alpha margin is below 2 percent".into());
    }
    let largest_component_pixels = largest_component(&alpha, width, height)?;
    let partial_alpha_ratio = f64::from(partial_alpha_pixels) / f64::from(visible_pixels);
    let largest_component_share = f64::from(largest_component_pixels) / f64::from(visible_pixels);
    if partial_alpha_ratio > 0.02 {
        return Err("pixel avatar partial alpha ratio exceeds 2 percent".into());
    }
    if largest_component_share < 0.95 {
        return Err("pixel avatar connected subject share is below 95 percent".into());
    }
    Ok(PixelPngInspection {
        width,
        height,
        visible_color_count: u16::try_from(visible_colors.len())
            .map_err(|_| "pixel avatar visible color count exceeds u16")?,
        alpha_report: PixelAlphaReportV1 {
            visible_pixels,
            partial_alpha_pixels,
            partial_alpha_ratio,
            largest_component_pixels,
            largest_component_share,
            bounds_left,
            bounds_top,
            bounds_right,
            bounds_bottom,
            margin_left: margins[0],
            margin_top: margins[1],
            margin_right: margins[2],
            margin_bottom: margins[3],
        },
    })
}

fn largest_component(alpha: &[u8], width: u32, height: u32) -> Result<u32, String> {
    let width = usize::try_from(width).map_err(|_| "pixel avatar width is invalid")?;
    let height = usize::try_from(height).map_err(|_| "pixel avatar height is invalid")?;
    if width.checked_mul(height) != Some(alpha.len()) {
        return Err("pixel avatar pixel buffer is invalid".into());
    }
    let mut visited = vec![false; alpha.len()];
    let mut largest = 0_u32;
    for index in 0..alpha.len() {
        if alpha[index] == 0 || visited[index] {
            continue;
        }
        let mut stack = vec![index];
        visited[index] = true;
        let mut component = 0_u32;
        while let Some(current) = stack.pop() {
            component += 1;
            let x = current % width;
            let y = current / width;
            let left = x.saturating_sub(1);
            let right = (x + 1).min(width - 1);
            let top = y.saturating_sub(1);
            let bottom = (y + 1).min(height - 1);
            for neighbor_y in top..=bottom {
                for neighbor_x in left..=right {
                    let neighbor = neighbor_y * width + neighbor_x;
                    if alpha[neighbor] > 0 && !visited[neighbor] {
                        visited[neighbor] = true;
                        stack.push(neighbor);
                    }
                }
            }
        }
        largest = largest.max(component);
    }
    Ok(largest)
}
