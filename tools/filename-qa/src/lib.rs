//! Exact production filename preparation, without a GUI or service execution.
#![allow(dead_code)] // Full-app entry points have no callers in this QA crate.

#[path = "../../../indexer/core/examples/support/captured_rows.rs"]
mod captured_rows;
mod cpu_clock;
#[path = "../../../src-tauri/src/file_policy.rs"]
mod file_policy;
#[path = "../../../src-tauri/src/filename_pattern.rs"]
mod filename_pattern;
#[path = "../../../src-tauri/src/keyboard_pattern.rs"]
mod keyboard_pattern;

// Configuration DTO only. Settings persistence and scope normalization are not
// exercised; filename preparation/search/catalog code is the production source.
mod settings {
    #[derive(Clone)]
    pub struct SearchScope {
        pub id: String,
        pub name: String,
        pub path: String,
        pub filename_enabled: bool,
        pub content_enabled: bool,
        pub external_allowed: bool,
    }
}

mod index {
    include!("../../../src-tauri/src/index.rs");
    use prism_ntfs_protocol::FileRecord;
    mod raw_mapper {
        use super::{path_key_text, Raw};
        use prism_ntfs_protocol::FileRecord;
        include!(concat!(env!("OUT_DIR"), "/raw.rs"));
    }
    include!("stages.rs");
    mod watch {
        include!("watch.rs");
    }
}

pub fn run() -> std::io::Result<()> {
    index::run_stages()
}
