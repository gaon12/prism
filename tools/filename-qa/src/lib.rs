//! Exact production filename preparation, without a GUI or service execution.
#![allow(dead_code)] // Full-app entry points have no callers in this QA crate.

#[path = "../../../indexer/core/examples/support/captured_rows.rs"]
mod captured_rows;
#[path = "../../../src-tauri/src/content_pipeline.rs"]
mod content_pipeline;
mod cpu_clock;
#[path = "../../../src-tauri/src/file_policy.rs"]
mod file_policy;
#[path = "../../../src-tauri/src/file_policy_fs.rs"]
mod file_policy_fs;
#[path = "../../../src-tauri/src/filename_pattern.rs"]
mod filename_pattern;
#[path = "../../../src-tauri/src/keyboard_pattern.rs"]
mod keyboard_pattern;
#[path = "../../../src-tauri/src/path_identity.rs"]
mod path_identity;
#[path = "../../../src-tauri/src/resource_policy.rs"]
mod resource_policy;

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
    pub(super) mod live_cpu {
        include!("live_cpu.rs");
    }
}

pub fn run() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "live-cpu") {
        return index::live_cpu::run(&args[1..]);
    }
    index::run_stages()
}
