//! Bounded authentic page rasterization. WebP bytes are always encoded locally;
//! source documents never become a browser URL or an external request.
use crate::collection_import::check;
use crate::collection_store::{error, ErrorCode, Page, PageKind, Result};
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::{io::Cursor, sync::atomic::AtomicBool};

pub(crate) fn webp(bytes: &[u8], edge: u32, cancel: &AtomicBool) -> Result<Vec<u8>> {
    check(cancel)?;
    if ![480, 1600].contains(&edge) {
        return Err(error(ErrorCode::Input));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| error(ErrorCode::Unsupported))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65_536);
    limits.max_image_height = Some(65_536);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| error(ErrorCode::Unsupported))?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > 64_000_000
        || decoder.total_bytes() > 256 << 20
    {
        return Err(error(ErrorCode::Limit));
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| error(ErrorCode::Unsupported))?;
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|_| error(ErrorCode::Unsupported))?;
    check(cancel)?;
    image.apply_orientation(orientation);
    // A long webtoon stays readable in the reader: constrain width, not its
    // longest edge. Thumbnails still fit their square well. Both paths have
    // explicit pixel bounds and never upscale the original.
    let (width, height) = (image.width(), image.height());
    let scale = if edge == 480 {
        (480.0 / f64::from(width.max(height))).min(1.0)
    } else {
        (1600.0 / f64::from(width))
            .min((32_000_000.0 / (f64::from(width) * f64::from(height))).sqrt())
            .min(1.0)
    };
    let image = if scale < 1.0 {
        image.resize_exact(
            (f64::from(width) * scale).floor().max(1.0) as u32,
            (f64::from(height) * scale).floor().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    }
    .to_rgba8();
    check(cancel)?;
    let mut output = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut output)
        .encode(
            &image,
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|_| error(ErrorCode::Unsupported))?;
    if output.len() > 16 << 20 {
        return Err(error(ErrorCode::Limit));
    }
    check(cancel)?;
    Ok(output)
}
pub(crate) fn render(page: &Page, bytes: &[u8], edge: u32, cancel: &AtomicBool) -> Result<Vec<u8>> {
    match page.location.kind {
        PageKind::Image => webp(bytes, edge, cancel),
        PageKind::Pdf | PageKind::Office => {
            let png = crate::ocr::render_pdf_page(bytes, page.location.number, edge, cancel)
                .map_err(|e| {
                    error(match e.code {
                        crate::ocr::Code::Cancelled => ErrorCode::Cancelled,
                        crate::ocr::Code::Busy => ErrorCode::Busy,
                        crate::ocr::Code::LimitExceeded => ErrorCode::Limit,
                        _ => ErrorCode::Unsupported,
                    })
                })?;
            webp(&png, edge, cancel)
        }
        PageKind::Text | PageKind::Epub => Err(error(ErrorCode::Unsupported)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_webp_preserves_first_image_pixels_and_fits_without_upscaling() {
        let image = image::RgbaImage::from_pixel(800, 400, image::Rgba([21, 113, 58, 255]));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = webp(png.get_ref(), 480, &AtomicBool::new(false)).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (480, 240));
        assert_eq!(decoded.get_pixel(240, 120).0, [21, 113, 58, 255]);
        let bytes = webp(png.get_ref(), 1600, &AtomicBool::new(false)).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 800);
        assert_eq!(
            webp(png.get_ref(), 480, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert!(webp(b"not an image", 480, &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn tall_webtoon_reader_preserves_readable_width_and_scrollable_height() {
        let image = image::RgbaImage::from_pixel(320, 5000, image::Rgba([40, 50, 60, 255]));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = webp(png.get_ref(), 1600, &AtomicBool::new(false)).unwrap();
        let reader = image::load_from_memory(&bytes).unwrap();
        assert_eq!((reader.width(), reader.height()), (320, 5000));
        let bytes = webp(png.get_ref(), 480, &AtomicBool::new(false)).unwrap();
        let thumb = image::load_from_memory(&bytes).unwrap();
        assert_eq!(thumb.height(), 480);
        assert!(thumb.width() < 40);
    }
}
