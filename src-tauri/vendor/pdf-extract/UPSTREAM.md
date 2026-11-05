# pdf-extract 0.12.1

Source: https://crates.io/crates/pdf-extract/0.12.1
Repository: https://github.com/jrmuizel/pdf-extract
Declared upstream license: MIT; author: Jeff Muizelaar and contributors.

The src directory and README are unchanged from the published crate.
Only Cargo.toml omits lopdf's wasm_js feature. Prism is a native desktop app;
that feature unnecessarily unifies getrandom features throughout Arrow/Lance
and invalidates the native build cache. PDF loading/extraction code is unchanged.
Both the application and standalone platform QA use this same source copy.

Original published crate SHA-256: e5c4820f5811e424ce037d08493ac87752ecc54339e0f1e40c3d17da93278861
