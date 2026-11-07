//! Local text recognition over already-authorized, bounded bytes. No network.
//! The document extractor remains responsible for source authorization/hash and
//! publication. Platform adapters never receive a source path or persist secrets.
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[cfg(any(target_os = "macos", test))]
#[path = "ocr_image.rs"]
mod native_image;
#[cfg(any(target_os = "macos", test))]
#[path = "ocr_work.rs"]
mod native_work;
#[cfg(windows)]
#[path = "ocr_windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "ocr_macos.rs"]
mod platform;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub enabled: bool,
    pub language: String,
    pub engine: Engine,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    #[default]
    Auto,
    System,
    PaddleVl16,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            language: "auto".into(),
            engine: Engine::Auto,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<()> {
        if self.language.is_empty()
            || self.language.len() > 48
            || !self
                .language
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(error(Code::InvalidInput));
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_source_pixels: u64,
    pub max_pixels: u64,
    pub max_dimension: u32,
    pub max_pages: usize,
    pub max_text_bytes: usize,
    pub max_duration: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 << 20,
            max_source_pixels: 64_000_000,
            max_pixels: 8_000_000,
            max_dimension: 3200,
            max_pages: 200,
            max_text_bytes: 32 << 20,
            max_duration: Duration::from_secs(180),
        }
    }
}
// Do not derive Debug: PDF passwords and source bytes are transient inputs.
pub enum Input<'a> {
    Image(&'a [u8]),
    Pdf {
        /// Authenticated, unencrypted bytes supplied by the document extractor.
        /// Native renderers need not implement every supported PDF cipher.
        bytes: &'a [u8],
        pages: &'a [u32],
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Code {
    Disabled,
    Unavailable,
    LanguageUnavailable,
    InvalidInput,
    CorruptDocument,
    LimitExceeded,
    Cancelled,
    TimedOut,
    Busy,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub code: Code,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(code: Code) -> Error {
    let message = match code {
        Code::Disabled => "Image and scanned-page text recognition is disabled.",
        Code::Unavailable => "The local text-recognition engine is unavailable.",
        Code::LanguageUnavailable => {
            "The selected OCR language is not installed in the operating system."
        }
        Code::InvalidInput => "The OCR request is invalid.",
        Code::CorruptDocument => "The image or PDF cannot be decoded.",
        Code::LimitExceeded => "The OCR input exceeds a worker resource budget.",
        Code::Cancelled => "Text recognition was cancelled.",
        Code::TimedOut => "The text-recognition deadline was reached.",
        Code::Busy => "A previous text-recognition operation is still finishing.",
        Code::Failed => "Local text recognition failed.",
    };
    Error {
        code,
        message: message.into(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub tag: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub engine: String,
    pub available: bool,
    pub pdf_available: bool,
    pub languages: Vec<Language>,
    pub reason: Option<Code>,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub number: u32,
    pub text: String,
}

static ACTIVE: Mutex<usize> = Mutex::new(0);
pub(crate) struct Lease;
impl Lease {
    pub(crate) fn acquire() -> Result<Arc<Self>> {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if *active >= 2 {
            return Err(error(Code::Busy));
        }
        *active += 1;
        Ok(Arc::new(Self))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
    }
}
pub(crate) struct Control<'a> {
    pub cancel: &'a AtomicBool,
    pub deadline: Instant,
    // Ownership-only on the process adapter; native async completions clone it.
    pub _lease: Arc<Lease>,
}
impl Control<'_> {
    pub(crate) fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            Err(error(Code::Cancelled))
        } else if Instant::now() >= self.deadline {
            Err(error(Code::TimedOut))
        } else {
            Ok(())
        }
    }
}
pub fn capabilities() -> Capabilities {
    #[cfg(any(windows, target_os = "macos"))]
    {
        platform::capabilities()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Capabilities {
            engine: "unavailable".into(),
            available: false,
            pdf_available: false,
            languages: vec![],
            reason: Some(Code::Unavailable),
        }
    }
}
pub fn recognize(
    input: Input<'_>,
    preferences: &Preferences,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Vec<Page>> {
    if cancel.load(Ordering::Acquire) {
        return Err(error(Code::Cancelled));
    }
    if !preferences.enabled {
        return Err(error(Code::Disabled));
    }
    preferences.validate()?;
    if limits.max_pixels == 0
        || limits.max_source_pixels == 0
        || limits.max_dimension == 0
        || limits.max_pages == 0
        || limits.max_text_bytes == 0
        || limits.max_duration.is_zero()
    {
        return Err(error(Code::InvalidInput));
    }
    let bytes = match &input {
        Input::Image(bytes) | Input::Pdf { bytes, .. } => bytes,
    };
    if bytes.is_empty() {
        return Err(error(Code::CorruptDocument));
    }
    if bytes.len() > limits.max_input_bytes {
        return Err(error(Code::LimitExceeded));
    }
    if let Input::Pdf { pages, .. } = &input {
        if pages.len() > limits.max_pages {
            return Err(error(Code::LimitExceeded));
        }
        if pages.contains(&0) || pages.windows(2).any(|p| p[0] >= p[1]) {
            return Err(error(Code::InvalidInput));
        }
        if pages.is_empty() {
            return Ok(vec![]);
        }
    }
    let control = Control {
        cancel,
        deadline: Instant::now()
            .checked_add(limits.max_duration)
            .ok_or_else(|| error(Code::InvalidInput))?,
        _lease: Lease::acquire()?,
    };
    control.check()?;
    let native = preferences.engine != Engine::PaddleVl16;
    if native {
        #[cfg(any(windows, target_os = "macos"))]
        {
            // A retry needs the same authorized immutable bytes. Only absence
            // of an engine/language permits Auto to try the local fallback;
            // cancellation, resource and document errors are never swallowed.
            let attempt = match &input {
                Input::Image(bytes) => Input::Image(bytes),
                Input::Pdf { bytes, pages } => Input::Pdf { bytes, pages },
            };
            match platform::recognize(attempt, preferences, limits, &control) {
                Ok(pages) => return Ok(pages),
                Err(cause)
                    if preferences.engine == Engine::Auto
                        && matches!(cause.code, Code::Unavailable | Code::LanguageUnavailable) =>
                {
                    if !crate::ocr_runtime::global()
                        .is_some_and(|runtime| runtime.status().available)
                    {
                        return Err(cause);
                    }
                }
                Err(cause) => return Err(cause),
            }
        }
        if preferences.engine == Engine::System {
            return Err(error(Code::Unavailable));
        }
    }
    crate::ocr_runtime::global()
        .ok_or_else(|| error(Code::Unavailable))?
        .recognize(input, limits, &control)
}
/// Render an already-authorized PDF page without recognizing text or changing
/// OCR preferences. Gallery callers provide bounded immutable bytes, never a
/// path; the existing native async lifetime/cancellation lease is shared.
pub(crate) fn render_pdf_page(
    bytes: &[u8],
    page: u32,
    dimension: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > 128 << 20 || page == 0 || !(1..=4096).contains(&dimension)
    {
        return Err(error(Code::InvalidInput));
    }
    let control = Control {
        cancel,
        deadline: Instant::now() + Duration::from_secs(20),
        _lease: Lease::acquire()?,
    };
    control.check()?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        platform::render_pdf_page(bytes, page, dimension, &control)
    }
    #[cfg(target_os = "linux")]
    {
        let launcher = std::env::current_exe().map_err(|_| error(Code::Unavailable))?;
        // A lib-test executable has no pre-GUI dispatch. Opt-in native tests
        // supply the exact-source platform QA host; release code cannot read
        // this override and always launches its own authenticated executable.
        #[cfg(test)]
        let launcher = std::env::var_os("PRISM_TEST_PDF_LAUNCHER")
            .map(std::path::PathBuf::from)
            .unwrap_or(launcher);
        crate::pdf_renderer::render(bytes, page, dimension, &launcher, cancel, control.deadline)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (bytes, page, dimension, control);
        Err(error(Code::Unavailable))
    }
}
/// Use only positive finite dimensions and floor each axis so neither the pixel
/// area nor longest edge exceeds its budget after fitting.
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
pub(crate) fn fitted(
    width: f64,
    height: f64,
    max_dimension: u32,
    max_pixels: u64,
    upscale: bool,
) -> Result<(u32, u32)> {
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > u32::MAX as f64
        || height > u32::MAX as f64
        || max_dimension == 0
        || max_pixels == 0
    {
        return Err(error(Code::LimitExceeded));
    }
    let mut scale = (f64::from(max_dimension) / width.max(height))
        .min((max_pixels as f64 / (width * height)).sqrt());
    if !upscale {
        scale = scale.min(1.0);
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err(error(Code::LimitExceeded));
    }
    let w = ((width * scale).floor() as u32).max(1);
    let h = ((height * scale).floor() as u32).max(1);
    if u64::from(w) * u64::from(h) > max_pixels {
        return Err(error(Code::LimitExceeded));
    }
    Ok((w, h))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_cancelled_and_invalid_requests_never_start_native_work() {
        let mut p = Preferences::default();
        let l = Limits::default();
        let c = AtomicBool::new(false);
        assert_eq!(
            recognize(Input::Image(b"not an image"), &p, &l, &c)
                .unwrap_err()
                .code,
            Code::Disabled
        );
        p.enabled = true;
        assert_eq!(
            recognize(Input::Image(b"x"), &p, &l, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            Code::Cancelled
        );
        p.language = "../../secret".into();
        assert_eq!(
            recognize(Input::Image(b"x"), &p, &l, &c).unwrap_err().code,
            Code::InvalidInput
        );
        p.language = "auto".into();
        assert_eq!(
            recognize(
                Input::Pdf {
                    bytes: b"x",
                    pages: &[2, 1]
                },
                &p,
                &l,
                &c
            )
            .unwrap_err()
            .code,
            Code::InvalidInput
        );
        assert_eq!(
            recognize(
                Input::Image(b"xx"),
                &p,
                &Limits {
                    max_input_bytes: 1,
                    ..l
                },
                &c
            )
            .unwrap_err()
            .code,
            Code::LimitExceeded
        );
    }
    #[test]
    fn scaling_bounds_wide_tall_and_extreme_images_without_upscaling() {
        for (w, h) in [
            (10000., 5000.),
            (5000., 10000.),
            (100000., 1.),
            (100., 100.),
        ] {
            let (x, y) = fitted(w, h, 3200, 8_000_000, false).unwrap();
            assert!(x <= 3200 && y <= 3200 && u64::from(x) * u64::from(y) <= 8_000_000);
        }
        assert_eq!(
            fitted(100., 50., 3200, 8_000_000, false).unwrap(),
            (100, 50)
        );
        assert!(fitted(f64::INFINITY, 20., 3200, 8_000_000, false).is_err());
        assert!(fitted(0., 20., 3200, 8_000_000, false).is_err());
    }
    #[test]
    fn admission_counts_the_last_operation_owner_and_checks_deadlines() {
        let first = Lease::acquire().unwrap();
        let completion_owner = Arc::clone(&first);
        let second = Lease::acquire().unwrap();
        assert!(matches!(
            Lease::acquire(),
            Err(Error {
                code: Code::Busy,
                ..
            })
        ));
        drop(first);
        assert!(matches!(
            Lease::acquire(),
            Err(Error {
                code: Code::Busy,
                ..
            })
        ));
        drop(completion_owner);
        let available = Lease::acquire().unwrap();
        let c = AtomicBool::new(false);
        let control = Control {
            cancel: &c,
            deadline: Instant::now(),
            _lease: available,
        };
        assert_eq!(control.check().unwrap_err().code, Code::TimedOut);
        c.store(true, Ordering::Release);
        assert_eq!(control.check().unwrap_err().code, Code::Cancelled);
        drop(control);
        drop(second);
    }
    #[test]
    fn subnormal_page_geometry_cannot_turn_infinite_scaling_into_one_pixel() {
        assert_eq!(
            fitted(f64::MIN_POSITIVE, f64::MIN_POSITIVE, 3200, 8_000_000, true)
                .unwrap_err()
                .code,
            Code::LimitExceeded
        );
    }
}
