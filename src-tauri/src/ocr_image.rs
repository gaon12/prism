//! Bounded decoding for the native Mac adapter. This module is also tested on
//! Windows/Linux; it does not claim to emulate Vision or Quartz.
use super::{error, fitted, Code, Limits, Result};
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::io::Cursor;

fn failed(value: image::ImageError) -> super::Error {
    error(if matches!(value, image::ImageError::Limits(_)) {
        Code::LimitExceeded
    } else {
        Code::CorruptDocument
    })
}

pub(super) fn decode(bytes: &[u8], limits: &Limits) -> Result<RgbaImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| error(Code::CorruptDocument))?;
    let mut allocation = image::Limits::default();
    allocation.max_alloc = Some(limits.max_source_pixels.saturating_mul(4).min(512 << 20));
    reader.limits(allocation);
    let mut decoder = reader.into_decoder().map_err(failed)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > limits.max_source_pixels
    {
        return Err(error(Code::LimitExceeded));
    }
    let orientation = decoder.orientation().map_err(failed)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
    image.apply_orientation(orientation);
    let (width, height) = fitted(
        image.width().into(),
        image.height().into(),
        limits.max_dimension,
        limits.max_pixels,
        false,
    )?;
    if image.width() != width || image.height() != height {
        image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    }
    let mut rgba = image.into_rgba8();
    // Hidden RGB under alpha zero must not become black OCR pixels. Keep a
    // plain opaque RGBA layout before handing the buffer to CoreGraphics.
    for pixel in rgba.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        }
        pixel[3] = 255;
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exif_rotation_precedes_fitting_and_preserves_pixel_orientation() {
        let image = decode(
            include_bytes!("../../tests/fixtures/ocr/exif-clockwise.jpg"),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(image.dimensions(), (64, 128));
        let top = image.get_pixel(32, 16);
        let bottom = image.get_pixel(32, 112);
        assert!(top[0] > 220 && top[2] < 30);
        assert!(bottom[2] > 220 && bottom[0] < 30);
    }
    #[test]
    fn transparent_fixture_is_flattened_and_bounds_precede_full_decode() {
        let bytes = include_bytes!("../../tests/fixtures/ocr/transparent.png");
        let image = decode(bytes, &Limits::default()).unwrap();
        assert!(image.pixels().all(|p| p[3] == 255));
        assert_eq!(image.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert!(image.pixels().any(|p| p[0] < 64 && p[1] < 64 && p[2] < 64));
        let limits = Limits {
            max_source_pixels: 1,
            ..Default::default()
        };
        assert_eq!(
            decode(bytes, &limits).unwrap_err().code,
            Code::LimitExceeded
        );
        let limits = Limits {
            max_dimension: 160,
            max_pixels: 12800,
            ..Default::default()
        };
        let image = decode(bytes, &limits).unwrap();
        assert!(image.width().max(image.height()) <= 160);
        assert!(u64::from(image.width()) * u64::from(image.height()) <= 12800);
        assert_eq!(
            decode(b"damaged", &Limits::default()).unwrap_err().code,
            Code::CorruptDocument
        );
    }
}
