#[path = "../../../src-tauri/src/document_hwpx.rs"]
mod document_hwpx;
use std::{
    io::Write,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cancel = AtomicBool::new(false);
    let control = document_hwpx::Control {
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    let markdown="# 프리즘 문서\n\n<!-- prism:fact; verified=false -->\n\n한국어 中文 日本語 English 😀\n\n**강조**와 *기울임*, `literal code`.\n\n근거[^S1], 다시[^S1]\n\n[^S1]: 원본.md · 2쪽 · 검증되지 않음\n\n| 항목 | 수치 |\n|---|---:|\n| 예산 | 120000원 |\n\n- 첫 항목\n- 둘째 항목\n\n![설명](file:///never/read.png)\n\n<script>no execution</script>\n";
    let result = document_hwpx::render("프리즘 문서", markdown, "ko", &control)
        .map_err(|e| format!("{e:?}"))?;
    let path = std::env::args_os().nth(1).ok_or("output path required")?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(&result.bytes)?;
    println!(
        "bytes={} omitted={} literalHtml={}",
        result.bytes.len(),
        result.omitted_images,
        result.literal_html
    );
    Ok(())
}
