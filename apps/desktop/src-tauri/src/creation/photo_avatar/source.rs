use super::store::NormalizedPhoto;
use image::{DynamicImage, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Cursor;

pub const MAX_PHOTO_COUNT: usize = 8;
pub const MAX_PHOTO_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_NORMALIZED_TOTAL_BYTES: usize = 40 * 1024 * 1024;
pub const MIN_PHOTO_DIMENSION: u32 = 256;
pub const MAX_PHOTO_DIMENSION: u32 = 4096;
pub const MAX_PHOTO_PIXELS: u64 = 16_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPhotoSource {
    pub bytes: Vec<u8>,
    pub claimed_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotoAvatarError {
    InvalidInput(String),
}

impl PhotoAvatarError {
    fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub fn is_invalid_input(&self) -> bool {
        matches!(self, Self::InvalidInput(_))
    }
}

impl std::fmt::Display for PhotoAvatarError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PhotoAvatarError {}

pub fn normalize_photo_sources(
    raw_sources: Vec<RawPhotoSource>,
) -> Result<Vec<NormalizedPhoto>, PhotoAvatarError> {
    if !(1..=MAX_PHOTO_COUNT).contains(&raw_sources.len()) {
        return Err(PhotoAvatarError::invalid_input(
            "photo avatar requires between one and eight photos",
        ));
    }

    let mut normalized = Vec::with_capacity(raw_sources.len());
    let mut normalized_hashes = HashSet::with_capacity(raw_sources.len());
    let mut total_bytes = 0_usize;
    for (ordinal, raw) in raw_sources.into_iter().enumerate() {
        if raw.bytes.is_empty() || raw.bytes.len() > MAX_PHOTO_BYTES {
            return Err(PhotoAvatarError::invalid_input(
                "photo avatar raw photo must be between one byte and 10 MiB",
            ));
        }
        let actual_sha256 = sha256_hex(&raw.bytes);
        if raw.claimed_sha256 != actual_sha256 {
            return Err(PhotoAvatarError::invalid_input(
                "photo avatar claimed SHA-256 does not match photo bytes",
            ));
        }
        let format = image::guess_format(&raw.bytes).map_err(|_| {
            PhotoAvatarError::invalid_input("photo avatar accepts JPEG or PNG only")
        })?;
        if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png) {
            return Err(PhotoAvatarError::invalid_input(
                "photo avatar accepts JPEG or PNG only",
            ));
        }
        let reader = ImageReader::with_format(Cursor::new(raw.bytes.as_slice()), format);
        let (width, height) = reader.into_dimensions().map_err(|_| {
            PhotoAvatarError::invalid_input("photo avatar image dimensions are invalid")
        })?;
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| {
                PhotoAvatarError::invalid_input("photo avatar image dimensions overflow")
            })?;
        if width < MIN_PHOTO_DIMENSION
            || height < MIN_PHOTO_DIMENSION
            || width > MAX_PHOTO_DIMENSION
            || height > MAX_PHOTO_DIMENSION
            || pixels > MAX_PHOTO_PIXELS
        {
            return Err(PhotoAvatarError::invalid_input(
                "photo avatar image dimensions are outside the supported limit",
            ));
        }
        let image = ImageReader::with_format(Cursor::new(raw.bytes.as_slice()), format)
            .decode()
            .map_err(|_| PhotoAvatarError::invalid_input("photo avatar image cannot be decoded"))?;
        let normalized_png = encode_rgba_png(image)?;
        total_bytes = validate_normalized_photo_capacity(normalized_png.len(), total_bytes)?;
        let sha256 = sha256_hex(&normalized_png);
        if !normalized_hashes.insert(sha256.clone()) {
            return Err(PhotoAvatarError::invalid_input(
                "photo avatar normalized photos must be distinct",
            ));
        }
        normalized.push(NormalizedPhoto {
            source_id: format!("source-{ordinal}-{}", &sha256[..12]),
            ordinal: ordinal as u32,
            normalized_png,
            sha256,
            width,
            height,
        });
    }
    Ok(normalized)
}

fn validate_normalized_photo_capacity(
    normalized_photo_bytes: usize,
    current_total_bytes: usize,
) -> Result<usize, PhotoAvatarError> {
    if normalized_photo_bytes == 0 || normalized_photo_bytes > MAX_PHOTO_BYTES {
        return Err(PhotoAvatarError::invalid_input(
            "photo avatar normalized photo must be between one byte and 10 MiB",
        ));
    }
    let total_bytes = current_total_bytes
        .checked_add(normalized_photo_bytes)
        .ok_or_else(|| {
            PhotoAvatarError::invalid_input("photo avatar normalized total bytes overflow")
        })?;
    if total_bytes > MAX_NORMALIZED_TOTAL_BYTES {
        return Err(PhotoAvatarError::invalid_input(
            "photo avatar normalized photos exceed 40 MiB",
        ));
    }
    Ok(total_bytes)
}

fn encode_rgba_png(image: DynamicImage) -> Result<Vec<u8>, PhotoAvatarError> {
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image.into_rgba8())
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|_| PhotoAvatarError::invalid_input("photo avatar PNG normalization failed"))?;
    Ok(bytes)
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_photo_sources, validate_normalized_photo_capacity, RawPhotoSource,
        MAX_NORMALIZED_TOTAL_BYTES, MAX_PHOTO_BYTES, MIN_PHOTO_DIMENSION,
    };
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use sha2::{Digest, Sha256};
    use std::io::Cursor;

    fn raw_with_color(
        format: ImageFormat,
        width: u32,
        height: u32,
        color: [u8; 4],
    ) -> RawPhotoSource {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, image::Rgba(color)));
        let mut bytes = Vec::new();
        if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image.to_rgb8())
                .write_to(&mut Cursor::new(&mut bytes), format)
                .unwrap();
        } else {
            image
                .write_to(&mut Cursor::new(&mut bytes), format)
                .unwrap();
        }
        RawPhotoSource {
            claimed_sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        }
    }

    fn raw(format: ImageFormat, width: u32, height: u32) -> RawPhotoSource {
        raw_with_color(format, width, height, [16, 32, 48, 255])
    }

    #[test]
    fn normalizes_one_to_eight_photos_in_order_as_rgba_png() {
        let single = normalize_photo_sources(vec![raw(ImageFormat::Png, 256, 256)]).unwrap();
        assert_eq!(single.len(), 1);
        let photos = normalize_photo_sources(vec![
            raw(ImageFormat::Jpeg, 800, 600),
            raw(ImageFormat::Png, 900, 700),
        ])
        .unwrap();

        assert_eq!(
            photos.iter().map(|photo| photo.ordinal).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(photos
            .iter()
            .all(|photo| photo.normalized_png.starts_with(b"\x89PNG\r\n\x1a\n")));
        assert_eq!(
            photos[0].source_id,
            format!("source-0-{}", &photos[0].sha256[..12])
        );
        assert_eq!(
            photos[1].source_id,
            format!("source-1-{}", &photos[1].sha256[..12])
        );
        let eight = (0..8)
            .map(|ordinal| raw_with_color(ImageFormat::Png, 256, 256, [ordinal, 0, 0, 255]))
            .collect();
        assert_eq!(normalize_photo_sources(eight).unwrap().len(), 8);
    }

    #[test]
    fn rejects_empty_or_more_than_eight_photos() {
        assert!(normalize_photo_sources(Vec::new())
            .unwrap_err()
            .is_invalid_input());
        let photos = (0..9).map(|_| raw(ImageFormat::Png, 256, 256)).collect();
        assert!(normalize_photo_sources(photos)
            .unwrap_err()
            .is_invalid_input());
    }

    #[test]
    fn rejects_mismatched_hash_unsupported_format_and_raw_oversize() {
        let mut mismatched = raw(ImageFormat::Png, 256, 256);
        mismatched.claimed_sha256 = "0".repeat(64);
        assert!(normalize_photo_sources(vec![mismatched])
            .unwrap_err()
            .is_invalid_input());

        let bytes = vec![0_u8; MAX_PHOTO_BYTES + 1];
        assert!(normalize_photo_sources(vec![RawPhotoSource {
            claimed_sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        }])
        .unwrap_err()
        .is_invalid_input());

        let bytes = b"not-an-image".to_vec();
        assert!(normalize_photo_sources(vec![RawPhotoSource {
            claimed_sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        }])
        .unwrap_err()
        .is_invalid_input());
    }

    #[test]
    fn rejects_invalid_dimensions_pixel_limits_and_duplicate_normalized_hashes() {
        assert!(normalize_photo_sources(vec![raw(
            ImageFormat::Png,
            MIN_PHOTO_DIMENSION - 1,
            MIN_PHOTO_DIMENSION,
        )])
        .unwrap_err()
        .is_invalid_input());
        assert!(
            normalize_photo_sources(vec![raw(ImageFormat::Png, 4097, MIN_PHOTO_DIMENSION)])
                .unwrap_err()
                .is_invalid_input()
        );
        assert!(
            normalize_photo_sources(vec![raw(ImageFormat::Png, 4096, 4096)])
                .unwrap_err()
                .is_invalid_input()
        );
        let duplicate = raw(ImageFormat::Png, 256, 256);
        assert!(normalize_photo_sources(vec![duplicate.clone(), duplicate])
            .unwrap_err()
            .is_invalid_input());
    }

    #[test]
    fn normalized_capacity_limits_accept_boundaries_and_reject_overflow() {
        assert!(validate_normalized_photo_capacity(MAX_PHOTO_BYTES, 0).is_ok());
        assert!(validate_normalized_photo_capacity(MAX_PHOTO_BYTES + 1, 0)
            .unwrap_err()
            .is_invalid_input());
        assert!(validate_normalized_photo_capacity(
            MAX_PHOTO_BYTES,
            MAX_NORMALIZED_TOTAL_BYTES - MAX_PHOTO_BYTES,
        )
        .is_ok());
        assert!(
            validate_normalized_photo_capacity(1, MAX_NORMALIZED_TOTAL_BYTES)
                .unwrap_err()
                .is_invalid_input()
        );
    }
}
