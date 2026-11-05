//! Apple's Vision and Quartz over bounded, already-authorized memory buffers.
//! No source URL/path, network client, shell or decrypted output file is used.
use super::{
    error, fitted, native_image, native_work, Capabilities, Code, Control, Input, Language, Limits,
    Page, Preferences, Result,
};
use block2::RcBlock;
use objc2::{
    rc::{autoreleasepool, Retained},
    runtime::AnyClass,
    sel, AnyThread,
};
use objc2_core_foundation::{CFData, CFMutableData, CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapInfo, CGColorRenderingIntent,
    CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGPDFBox, CGPDFDocument, CGPDFPage,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSLocale, NSObjectProtocol, NSString};
use objc2_image_io::CGImageDestination;
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestProgressProviding,
    VNRequestTextRecognitionLevel,
};
use std::{
    ptr::{null, null_mut, NonNull},
    sync::{atomic::Ordering, Arc},
    time::Instant,
};

fn engine() -> Result<Retained<VNRecognizeTextRequest>> {
    // Tauri can run on a system older than text recognition. Do not send a
    // message to an absent class and claim that its language model is installed.
    if AnyClass::get(c"VNRecognizeTextRequest").is_none() {
        return Err(error(Code::Unavailable));
    }
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    Ok(request)
}

fn languages(request: &VNRecognizeTextRequest) -> Result<Retained<NSArray<NSString>>> {
    if !request.respondsToSelector(sel!(supportedRecognitionLanguagesAndReturnError:)) {
        return Err(error(Code::Unavailable));
    }
    // SAFETY: a live request, configured for accurate recognition, on this worker.
    let values = unsafe { request.supportedRecognitionLanguagesAndReturnError() }
        .map_err(|_| error(Code::Unavailable))?;
    if values.is_empty() || values.len() > 256 {
        return Err(error(Code::Unavailable));
    }
    Ok(values)
}

fn language_tag(value: &str, available: &[String]) -> Option<String> {
    let value = value.replace('_', "-").to_ascii_lowercase();
    available
        .iter()
        .find(|tag| tag.eq_ignore_ascii_case(&value))
        .or_else(|| {
            available
                .iter()
                .filter(|tag| value.starts_with(&(tag.to_ascii_lowercase() + "-")))
                .max_by_key(|tag| tag.len())
        })
        .or_else(|| {
            (!value.contains('-'))
                .then(|| {
                    available.iter().find(|tag| {
                        tag.split('-')
                            .next()
                            .is_some_and(|base| base.eq_ignore_ascii_case(&value))
                    })
                })
                .flatten()
        })
        .cloned()
}

pub(super) fn capabilities() -> Capabilities {
    let result = autoreleasepool(|_| -> Result<Vec<Language>> {
        let request = engine()?;
        let available = languages(&request)?;
        let locale = NSLocale::currentLocale();
        Ok(available
            .iter()
            .map(|tag| Language {
                tag: tag.to_string(),
                name: locale.localizedStringForLocaleIdentifier(&tag).to_string(),
            })
            .collect())
    });
    match result {
        Ok(languages) => Capabilities {
            engine: "macNative".into(),
            available: true,
            pdf_available: true,
            languages,
            reason: None,
        },
        Err(cause) => Capabilities {
            engine: "macNative".into(),
            available: false,
            pdf_available: true,
            languages: vec![],
            reason: Some(cause.code),
        },
    }
}

fn configured(
    preferences: &Preferences,
    budget: &native_work::Budget,
) -> Result<Retained<VNRecognizeTextRequest>> {
    budget.check()?;
    let request = engine()?;
    let available: Vec<_> = languages(&request)?
        .iter()
        .map(|value| value.to_string())
        .collect();
    let selected = if preferences.language == "auto" {
        let mut selected = Vec::new();
        for value in NSLocale::preferredLanguages().iter() {
            let value = value.to_string();
            if let Some(tag) = language_tag(&value, &available)
                .or_else(|| language_tag(value.split('-').next().unwrap_or(&value), &available))
            {
                if !selected.contains(&tag) {
                    selected.push(tag);
                }
            }
        }
        for value in unsafe { request.recognitionLanguages() }.iter() {
            if let Some(tag) = language_tag(&value.to_string(), &available) {
                if !selected.contains(&tag) {
                    selected.push(tag);
                }
            }
        }
        if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
            request.setAutomaticallyDetectsLanguage(true);
        }
        selected
    } else {
        vec![language_tag(&preferences.language, &available)
            .ok_or_else(|| error(Code::LanguageUnavailable))?]
    };
    if !selected.is_empty() {
        let values: Vec<_> = selected.iter().map(|tag| NSString::from_str(tag)).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&values));
    }
    let cancel = Arc::clone(&budget.cancel);
    let deadline = budget.deadline;
    let progress = RcBlock::new(
        move |request: NonNull<VNRequest>, _: f64, _: *mut NSError| {
            if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                // Vision supplies this live request to its progress callback, which
                // may run on another queue. Retain no ObjC object in the closure;
                // cancellation is the only message sent through this borrowed pointer.
                unsafe {
                    request.as_ref().cancel();
                }
            }
        },
    );
    // Vision copies the block. Its captures are owned thread-safe budget state.
    unsafe {
        request.setProgressHandler(RcBlock::as_ptr(&progress));
    }
    Ok(request)
}

fn read_text(
    image: &CGImage,
    request: &VNRecognizeTextRequest,
    remaining: usize,
    budget: &native_work::Budget,
) -> Result<String> {
    let requests = NSArray::<VNRequest>::from_slice(&[request]);
    let options = NSDictionary::new();
    // The image's provider owns its pixels; handler/request/image all remain
    // on the worker until synchronous performRequests has actually returned.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            image,
            &options,
        )
    };
    let result = handler.performRequests_error(&requests);
    budget.check()?;
    result.map_err(|_| error(Code::Failed))?;
    let observations = request.results().ok_or_else(|| error(Code::Failed))?;
    if observations.len() > 100_000 {
        return Err(error(Code::LimitExceeded));
    }
    let mut text = String::new();
    for observation in observations.iter() {
        budget.check()?;
        if let Some(candidate) = observation.topCandidates(1).firstObject() {
            let line = candidate.string().to_string();
            let separator = usize::from(!text.is_empty());
            if text
                .len()
                .saturating_add(separator)
                .saturating_add(line.len())
                > remaining
            {
                return Err(error(Code::LimitExceeded));
            }
            if separator != 0 {
                text.push('\n');
            }
            text.push_str(&line);
        }
    }
    Ok(text)
}

fn cg_image(pixels: image::RgbaImage) -> Result<CFRetained<CGImage>> {
    let (width, height) = pixels.dimensions();
    let data = CFData::from_bytes(pixels.as_raw());
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(|| error(Code::Failed))?;
    let color = CGColorSpace::new_device_rgb().ok_or_else(|| error(Code::Unavailable))?;
    let info = CGBitmapInfo::from_bits_retain(
        CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0,
    );
    // RGBA was flattened opaque, with exactly width*height*4 bytes. CGImage
    // retains the provider, which retains its CFData copy after this function.
    unsafe {
        CGImage::new(
            width as usize,
            height as usize,
            8,
            32,
            width as usize * 4,
            Some(&color),
            info,
            Some(&provider),
            null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| error(Code::Failed))
}

fn pdf(bytes: &[u8]) -> Result<CFRetained<CGPDFDocument>> {
    let data = CFData::from_bytes(bytes);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(|| error(Code::Failed))?;
    let document = CGPDFDocument::with_provider(Some(&provider))
        .ok_or_else(|| error(Code::CorruptDocument))?;
    if !CGPDFDocument::is_unlocked(Some(&document)) {
        return Err(error(Code::CorruptDocument));
    }
    Ok(document)
}

fn raster(
    document: &CGPDFDocument,
    number: u32,
    limits: &Limits,
    budget: &native_work::Budget,
) -> Result<CFRetained<CGImage>> {
    budget.check()?;
    if number == 0 || number as usize > CGPDFDocument::number_of_pages(Some(document)) {
        return Err(error(Code::InvalidInput));
    }
    let page = CGPDFDocument::page(Some(document), number as usize)
        .ok_or_else(|| error(Code::CorruptDocument))?;
    let bounds = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
    let rotation = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(360);
    if !bounds.origin.x.is_finite() || !bounds.origin.y.is_finite() || rotation % 90 != 0 {
        return Err(error(Code::CorruptDocument));
    }
    let (width, height) = if rotation % 180 == 0 {
        (bounds.size.width, bounds.size.height)
    } else {
        (bounds.size.height, bounds.size.width)
    };
    let (width, height) = fitted(width, height, limits.max_dimension, limits.max_pixels, true)?;
    let color = CGColorSpace::new_device_rgb().ok_or_else(|| error(Code::Unavailable))?;
    let info = CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0;
    // Quartz owns this bounded allocation. No Rust buffer can be dropped while
    // an asynchronous cancellation caller is already returning to the extractor.
    let context = unsafe {
        CGBitmapContextCreate(
            null_mut(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            Some(&color),
            info,
        )
    }
    .ok_or_else(|| error(Code::LimitExceeded))?;
    let target = CGRect {
        origin: CGPoint { x: 0., y: 0. },
        size: CGSize {
            width: width.into(),
            height: height.into(),
        },
    };
    CGContext::set_rgb_fill_color(Some(&context), 1., 1., 1., 1.);
    CGContext::fill_rect(Some(&context), target);
    let transform = CGPDFPage::drawing_transform(Some(&page), CGPDFBox::CropBox, target, 0, true);
    if ![
        transform.a,
        transform.b,
        transform.c,
        transform.d,
        transform.tx,
        transform.ty,
    ]
    .iter()
    .all(|value| value.is_finite())
    {
        return Err(error(Code::CorruptDocument));
    }
    CGContext::concat_ctm(Some(&context), transform);
    // The drawing transform maps the box but does not itself establish a clip.
    // Keep content outside the crop out of any rounding/letterbox margin too.
    // https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/drawingwithquartz2d/dq_pdf/dq_pdf.html
    CGContext::clip_to_rect(Some(&context), bounds);
    CGContext::draw_pdf_page(Some(&context), Some(&page));
    budget.check()?;
    CGBitmapContextCreateImage(Some(&context)).ok_or_else(|| error(Code::Failed))
}

pub(super) fn recognize(
    input: Input<'_>,
    preferences: &Preferences,
    limits: &Limits,
    control: &Control<'_>,
) -> Result<Vec<Page>> {
    let (bytes, pages) = match input {
        Input::Image(bytes) => (bytes.to_vec(), None),
        Input::Pdf { bytes, pages } => (bytes.to_vec(), Some(pages.to_vec())),
    };
    let preferences = preferences.clone();
    let limits = limits.clone();
    native_work::run(control, move |budget| {
        autoreleasepool(|_| {
            budget.check()?;
            // Fail missing engines/languages before decoding or rendering; a
            // document's pages reuse this request sequentially on one worker.
            let request = configured(&preferences, &budget)?;
            if let Some(pages) = pages {
                let document = pdf(&bytes)?;
                if pages.iter().any(|page| {
                    *page == 0 || *page as usize > CGPDFDocument::number_of_pages(Some(&document))
                }) {
                    return Err(error(Code::InvalidInput));
                }
                let mut output = Vec::new();
                let mut used = 0usize;
                for number in pages {
                    let image = raster(&document, number, &limits, &budget)?;
                    let text = read_text(
                        &image,
                        &request,
                        limits.max_text_bytes.saturating_sub(used),
                        &budget,
                    )?;
                    used += text.len();
                    output.push(Page { number, text });
                }
                Ok(output)
            } else {
                let image = cg_image(native_image::decode(&bytes, &limits)?)?;
                budget.check()?;
                let text = read_text(&image, &request, limits.max_text_bytes, &budget)?;
                Ok(vec![Page { number: 1, text }])
            }
        })
    })
}

pub(super) fn render_pdf_page(
    bytes: &[u8],
    page: u32,
    dimension: u32,
    control: &Control<'_>,
) -> Result<Vec<u8>> {
    let bytes = bytes.to_vec();
    native_work::run(control, move |budget| {
        autoreleasepool(|_| {
            let document = pdf(&bytes)?;
            let limits = Limits {
                max_dimension: dimension,
                ..Default::default()
            };
            let image = raster(&document, page, &limits, &budget)?;
            let data = CFMutableData::new(None, 0).ok_or_else(|| error(Code::Failed))?;
            let kind = CFString::from_str("public.png");
            let destination = unsafe { CGImageDestination::with_data(&data, &kind, 1, None) }
                .ok_or_else(|| error(Code::Failed))?;
            unsafe {
                destination.add_image(&image, None);
            }
            // Only our owned data, a valid image and no caller properties were
            // passed to this destination; finalize remains on the same worker.
            if !unsafe { destination.finalize() } {
                return Err(error(Code::Failed));
            }
            budget.check()?;
            if data.len() > 64 << 20 {
                return Err(error(Code::LimitExceeded));
            }
            Ok(data.to_vec())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn regional_language_matching_keeps_script_specificity() {
        let available: Vec<String> = ["en-US", "ko-KR", "zh-Hans", "zh-Hant"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(language_tag("ko_KR", &available).as_deref(), Some("ko-KR"));
        assert_eq!(language_tag("ko", &available).as_deref(), Some("ko-KR"));
        assert_eq!(
            language_tag("zh-Hant-HK", &available).as_deref(),
            Some("zh-Hant")
        );
        assert_eq!(language_tag("en-GB", &available), None);
        assert_eq!(language_tag("zz-Unknown", &available), None);
    }
    #[test]
    #[ignore = "requires actual macOS Quartz; no Vision model or OCR opt-in is required"]
    fn macos_quartz_preserves_pdf_crop_rotation_and_preview_without_ocr() {
        assert!(!Preferences::default().enabled);
        let cancel = AtomicBool::new(false);
        let bytes = include_bytes!("../../tests/fixtures/ocr/crop-clockwise.pdf");
        let png = super::super::render_pdf_page(bytes, 1, 200, &cancel).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (100, 200));
        let top = image.get_pixel(50, 25);
        let bottom = image.get_pixel(50, 175);
        assert!(top[0] > 220 && top[1] < 30 && top[2] < 30);
        assert!(bottom[2] > 220 && bottom[0] < 30 && bottom[1] < 30);
        assert_eq!(
            super::super::render_pdf_page(bytes, 2, 200, &cancel)
                .unwrap_err()
                .code,
            Code::InvalidInput
        );
        assert_eq!(
            super::super::render_pdf_page(b"damaged", 1, 200, &cancel)
                .unwrap_err()
                .code,
            Code::CorruptDocument
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            super::super::render_pdf_page(bytes, 1, 200, &cancelled)
                .unwrap_err()
                .code,
            Code::Cancelled
        );
    }
}
