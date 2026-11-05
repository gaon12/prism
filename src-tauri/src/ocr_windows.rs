//! Windows.Media.Ocr and Windows.Data.Pdf. All streams/bitmaps stay in memory.
use super::{
    error, fitted, Capabilities, Code, Control, Error, Input, Language as OcrLanguage, Limits,
    Page, Preferences, Result,
};
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use windows::{
    core::{Interface, HSTRING},
    Data::Pdf::{PdfDocument, PdfPageRenderOptions},
    Globalization::Language,
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapFrame, BitmapPixelFormat, BitmapTransform,
        ColorManagementMode, ExifOrientationMode, SoftwareBitmap,
    },
    Media::Ocr::OcrEngine,
    Storage::Streams::{Buffer, DataReader, DataWriter, InMemoryRandomAccessStream},
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};
use windows_future::{
    AsyncActionCompletedHandler, AsyncOperationCompletedHandler,
    AsyncOperationWithProgressCompletedHandler, AsyncStatus, IAsyncAction, IAsyncInfo,
    IAsyncOperation, IAsyncOperationWithProgress,
};

trait NativeOperation: Interface {
    type Output;
    fn completed(&self, callback: impl Fn() + Send + Sync + 'static) -> windows::core::Result<()>;
    fn results(&self) -> windows::core::Result<Self::Output>;
}
impl<T: windows::core::RuntimeType> NativeOperation for IAsyncOperation<T> {
    type Output = T;
    fn completed(&self, callback: impl Fn() + Send + Sync + 'static) -> windows::core::Result<()> {
        self.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
            callback();
            Ok(())
        }))
    }
    fn results(&self) -> windows::core::Result<T> {
        self.GetResults()
    }
}
impl NativeOperation for IAsyncAction {
    type Output = ();
    fn completed(&self, callback: impl Fn() + Send + Sync + 'static) -> windows::core::Result<()> {
        self.SetCompleted(&AsyncActionCompletedHandler::new(move |_, _| {
            callback();
            Ok(())
        }))
    }
    fn results(&self) -> windows::core::Result<()> {
        self.GetResults()
    }
}
impl NativeOperation for IAsyncOperationWithProgress<u32, u32> {
    type Output = u32;
    fn completed(&self, callback: impl Fn() + Send + Sync + 'static) -> windows::core::Result<()> {
        self.SetCompleted(&AsyncOperationWithProgressCompletedHandler::new(
            move |_, _| {
                callback();
                Ok(())
            },
        ))
    }
    fn results(&self) -> windows::core::Result<u32> {
        self.GetResults()
    }
}

struct Apartment(PhantomData<Rc<()>>);
thread_local! {
    // WinRT activation factories are cached. Keep a worker's apartment alive
    // across capability queries and successive pages instead of tearing it down
    // between operations that share those factories.
    static APARTMENT: Result<Apartment> = Apartment::new();
}
impl Apartment {
    fn ensure() -> Result<()> {
        APARTMENT.with(|value| value.as_ref().map(|_| ()).map_err(Clone::clone))
    }
    fn new() -> Result<Self> {
        // Callers run on a document/IPC blocking worker, never the UI STA.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED).map_err(|_| error(Code::Unavailable))?;
        }
        Ok(Self(PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
fn failed(_: windows::core::Error) -> Error {
    error(Code::Failed)
}
fn corrupt(_: windows::core::Error) -> Error {
    error(Code::CorruptDocument)
}

/// A cancelled request may stop waiting before the OS completes its operation.
/// Its completion delegate retains a lease so repeated retries still cannot
/// start unbounded native work. No password, image or text enters this delegate.
fn wait<A: NativeOperation>(operation: A, control: &Control<'_>) -> Result<A::Output> {
    let keep = Arc::new(Mutex::new(Some(Arc::clone(&control.lease))));
    let on_done = Arc::clone(&keep);
    operation
        .completed(move || {
            drop(on_done.lock().unwrap_or_else(|e| e.into_inner()).take());
        })
        .map_err(failed)?;
    let info = operation.cast::<IAsyncInfo>().map_err(failed)?;
    loop {
        if let Err(value) = control.check() {
            let _ = info.Cancel();
            return Err(value);
        }
        match info.Status().map_err(failed)? {
            AsyncStatus::Started => thread::sleep(Duration::from_millis(15)),
            status => {
                // The status can become terminal just before the completion
                // delegate is dispatched. Release the same optional lease here.
                drop(keep.lock().unwrap_or_else(|e| e.into_inner()).take());
                return if status == AsyncStatus::Canceled {
                    Err(error(Code::Cancelled))
                } else {
                    operation.results().map_err(failed)
                };
            }
        }
    }
}
// Keep engine failures separate from decoder/renderer failures. Cancellation and
// deadline errors retain their own codes instead of becoming damaged-document.
fn decode<A: NativeOperation>(operation: A, control: &Control<'_>) -> Result<A::Output> {
    wait(operation, control).map_err(|value| {
        if value.code == Code::Failed {
            error(Code::CorruptDocument)
        } else {
            value
        }
    })
}
fn stream(bytes: &[u8], control: &Control<'_>) -> Result<InMemoryRandomAccessStream> {
    control.check()?;
    let stream = InMemoryRandomAccessStream::new().map_err(failed)?;
    let writer = DataWriter::new().map_err(failed)?;
    writer.WriteBytes(bytes).map_err(failed)?;
    let buffer = writer.DetachBuffer().map_err(failed)?;
    let written = wait(stream.WriteAsync(&buffer).map_err(failed)?, control)?;
    if written as usize != bytes.len() {
        return Err(error(Code::Failed));
    }
    stream.Seek(0).map_err(failed)?;
    Ok(stream)
}
fn languages() -> Result<Vec<OcrLanguage>> {
    let available =
        OcrEngine::AvailableRecognizerLanguages().map_err(|_| error(Code::Unavailable))?;
    let mut result = Vec::new();
    for index in 0..available.Size().map_err(failed)? {
        let item = available.GetAt(index).map_err(failed)?;
        result.push(OcrLanguage {
            tag: item.LanguageTag().map_err(failed)?.to_string(),
            name: item.DisplayName().map_err(failed)?.to_string(),
        });
    }
    Ok(result)
}
fn engine(preferences: &Preferences) -> Result<OcrEngine> {
    if preferences.language == "auto" {
        if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
            return Ok(engine);
        }
        if let Some(first) = languages()?.first() {
            let language = Language::CreateLanguage(&HSTRING::from(&first.tag)).map_err(failed)?;
            return OcrEngine::TryCreateFromLanguage(&language)
                .map_err(|_| error(Code::LanguageUnavailable));
        }
        return Err(error(Code::LanguageUnavailable));
    }
    let available = languages()?;
    // Match a selected base tag (e.g. ko) to an installed regional tag, or vice
    // versa. An explicit unsupported language never silently changes to another.
    let selected = available
        .iter()
        .find(|l| l.tag.eq_ignore_ascii_case(&preferences.language))
        .or_else(|| {
            available.iter().find(|l| {
                l.tag
                    .split('-')
                    .next()
                    .unwrap_or("")
                    .eq_ignore_ascii_case(&preferences.language)
                    || preferences
                        .language
                        .split('-')
                        .next()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(&l.tag)
            })
        })
        .ok_or_else(|| error(Code::LanguageUnavailable))?;
    let language = Language::CreateLanguage(&HSTRING::from(&selected.tag)).map_err(failed)?;
    OcrEngine::TryCreateFromLanguage(&language).map_err(|_| error(Code::LanguageUnavailable))
}
pub(super) fn capabilities() -> Capabilities {
    let result = (|| -> Result<_> {
        Apartment::ensure()?;
        let available = languages()?;
        let _ = engine(&Preferences::default())?;
        // Drop the WinRT temporary before Apartment uninitializes this worker.
        // A tail-expression temporary would outlive the apartment and its final
        // COM Release can crash an otherwise successful capability query.
        let pdf_available = PdfPageRenderOptions::new().is_ok();
        Ok((available, pdf_available))
    })();
    match result {
        Ok((languages, pdf_available)) => Capabilities {
            engine: "windowsNative".into(),
            available: true,
            pdf_available,
            languages,
            reason: None,
        },
        Err(value) => Capabilities {
            engine: "windowsNative".into(),
            available: false,
            pdf_available: false,
            languages: vec![],
            reason: Some(value.code),
        },
    }
}
fn bitmap(
    frame: &BitmapFrame,
    limits: &Limits,
    dimension: u32,
    control: &Control<'_>,
) -> Result<SoftwareBitmap> {
    control.check()?;
    let width = frame.PixelWidth().map_err(corrupt)?;
    let height = frame.PixelHeight().map_err(corrupt)?;
    if u64::from(width) * u64::from(height) > limits.max_source_pixels {
        return Err(error(Code::LimitExceeded));
    }
    let (width, height) = fitted(
        width.into(),
        height.into(),
        dimension,
        limits.max_pixels,
        false,
    )?;
    let transform = BitmapTransform::new().map_err(failed)?;
    transform.SetScaledWidth(width).map_err(failed)?;
    transform.SetScaledHeight(height).map_err(failed)?;
    let image = decode(
        frame
            .GetSoftwareBitmapTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Straight,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::DoNotColorManage,
            )
            .map_err(corrupt)?,
        control,
    )?;
    let width = image.PixelWidth().map_err(failed)?;
    let height = image.PixelHeight().map_err(failed)?;
    if width <= 0
        || height <= 0
        || u64::from(width as u32) * u64::from(height as u32) > limits.max_pixels
    {
        return Err(error(Code::LimitExceeded));
    }
    // Composite transparent images over white rather than interpreting their
    // hidden RGB values as an opaque black page. The allocation is pixel-bounded.
    let length = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or_else(|| error(Code::LimitExceeded))?;
    let buffer = Buffer::Create(u32::try_from(length).map_err(|_| error(Code::LimitExceeded))?)
        .map_err(failed)?;
    image.CopyToBuffer(&buffer).map_err(failed)?;
    let mut pixels = vec![0; length];
    DataReader::FromBuffer(&buffer)
        .map_err(failed)?
        .ReadBytes(&mut pixels)
        .map_err(failed)?;
    for row in pixels.chunks_mut(width as usize * 4) {
        control.check()?;
        for p in row.chunks_exact_mut(4) {
            let alpha = u16::from(p[3]);
            for c in &mut p[..3] {
                *c = ((u16::from(*c) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
            p[3] = 255;
        }
    }
    let writer = DataWriter::new().map_err(failed)?;
    writer.WriteBytes(&pixels).map_err(failed)?;
    SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &writer.DetachBuffer().map_err(failed)?,
        BitmapPixelFormat::Bgra8,
        width,
        height,
        BitmapAlphaMode::Ignore,
    )
    .map_err(failed)
}
fn page_text(
    image: &SoftwareBitmap,
    engine: &OcrEngine,
    remaining: usize,
    control: &Control<'_>,
) -> Result<String> {
    let result = wait(engine.RecognizeAsync(image).map_err(failed)?, control)?;
    let lines = result.Lines().map_err(failed)?;
    let mut text = String::new();
    for index in 0..lines.Size().map_err(failed)? {
        control.check()?;
        let line = lines
            .GetAt(index)
            .map_err(failed)?
            .Text()
            .map_err(failed)?
            .to_string();
        let length = text
            .len()
            .checked_add(line.len())
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| error(Code::LimitExceeded))?;
        if length > remaining {
            return Err(error(Code::LimitExceeded));
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
    }
    Ok(text)
}
pub(super) fn recognize(
    input: Input<'_>,
    preferences: &Preferences,
    limits: &Limits,
    control: &Control<'_>,
) -> Result<Vec<Page>> {
    Apartment::ensure()?;
    control.check()?;
    let engine = engine(preferences)?;
    let dimension = limits
        .max_dimension
        .min(OcrEngine::MaxImageDimension().map_err(failed)?);
    let mut pages = Vec::new();
    let mut text_bytes = 0;
    match input {
        Input::Image(bytes) => {
            let source = stream(bytes, control)?;
            let decoder = decode(
                BitmapDecoder::CreateAsync(&source).map_err(corrupt)?,
                control,
            )?;
            let count = decoder.FrameCount().map_err(corrupt)?;
            if count as usize > limits.max_pages {
                return Err(error(Code::LimitExceeded));
            }
            for index in 0..count {
                let frame = decode(decoder.GetFrameAsync(index).map_err(corrupt)?, control)?;
                let image = bitmap(&frame, limits, dimension, control)?;
                let text = page_text(
                    &image,
                    &engine,
                    limits.max_text_bytes.saturating_sub(text_bytes),
                    control,
                )?;
                text_bytes += text.len();
                pages.push(Page {
                    number: index + 1,
                    text,
                });
            }
        }
        Input::Pdf {
            bytes,
            pages: wanted,
        } => {
            let source = stream(bytes, control)?;
            let operation = PdfDocument::LoadFromStreamAsync(&source).map_err(corrupt)?;
            let document = decode(operation, control)?;
            let count = document.PageCount().map_err(corrupt)?;
            if wanted.iter().any(|page| *page > count) {
                return Err(error(Code::InvalidInput));
            }
            for number in wanted {
                control.check()?;
                let page = document.GetPage(number - 1).map_err(corrupt)?;
                let size = page.Size().map_err(corrupt)?;
                let (width, height) = fitted(
                    size.Width.into(),
                    size.Height.into(),
                    dimension,
                    limits.max_pixels,
                    true,
                )?;
                let render = PdfPageRenderOptions::new().map_err(failed)?;
                render.SetDestinationWidth(width).map_err(failed)?;
                render.SetDestinationHeight(height).map_err(failed)?;
                let rendered = InMemoryRandomAccessStream::new().map_err(failed)?;
                decode(
                    page.RenderWithOptionsToStreamAsync(&rendered, &render)
                        .map_err(corrupt)?,
                    control,
                )?;
                if rendered.Size().map_err(failed)? > limits.max_pixels.saturating_mul(8) {
                    return Err(error(Code::LimitExceeded));
                }
                rendered.Seek(0).map_err(failed)?;
                let decoder = decode(
                    BitmapDecoder::CreateAsync(&rendered).map_err(corrupt)?,
                    control,
                )?;
                let frame = decode(decoder.GetFrameAsync(0).map_err(corrupt)?, control)?;
                let image = bitmap(&frame, limits, dimension, control)?;
                let text = page_text(
                    &image,
                    &engine,
                    limits.max_text_bytes.saturating_sub(text_bytes),
                    control,
                )?;
                text_bytes += text.len();
                pages.push(Page {
                    number: *number,
                    text,
                });
            }
        }
    }
    control.check()?;
    Ok(pages)
}
