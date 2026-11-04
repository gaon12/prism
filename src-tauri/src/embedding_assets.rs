//! Immutable upstream CPU runtime inventory. Archive bytes, hashes and layouts
//! are pinned to the same release as the embedding protocol implementation.
//! Source: https://github.com/ggml-org/llama.cpp/releases/tag/b11960
pub const RUNTIME_VERSION: &str = "b11960";

#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub filename: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}
#[derive(Clone, Copy, Debug)]
pub struct RuntimePackage {
    pub target: &'static str,
    pub asset: Asset,
    pub format: ArchiveFormat,
    pub executable: &'static str,
}
#[cfg(test)]
const BASE: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b11960/";
macro_rules! package {
    ($target:literal, $file:literal, $size:literal, $hash:literal, $format:ident, $exe:literal) => {
        RuntimePackage {
            target: $target,
            asset: Asset {
                filename: $file,
                url: concat!(
                    "https://github.com/ggml-org/llama.cpp/releases/download/b11960/",
                    $file
                ),
                bytes: $size,
                sha256: $hash,
            },
            format: ArchiveFormat::$format,
            executable: $exe,
        }
    };
}
pub const PACKAGES: [RuntimePackage; 5] = [
    package!(
        "windows-x86_64",
        "llama-b11960-bin-win-cpu-x64.zip",
        24_407_745,
        "73f962f82850d244db47eaacb9d913637027b82399124324298576d58c5f4287",
        Zip,
        "llama-server.exe"
    ),
    package!(
        "macos-aarch64",
        "llama-b11960-bin-macos-arm64.tar.gz",
        16_465_074,
        "58e5a5e1d7c2bf66cedd9c29e0323f3e570660a70a1e2156d32eed08f4cc1e3c",
        TarGz,
        "llama-server"
    ),
    package!(
        "macos-x86_64",
        "llama-b11960-bin-macos-x64.tar.gz",
        12_205_764,
        "d1e6c9ddcaed9a3b1bb8f3d946bdb3481c7b919357a4029dcb747762397f1da5",
        TarGz,
        "llama-server"
    ),
    package!(
        "linux-x86_64",
        "llama-b11960-bin-ubuntu-x64.tar.gz",
        22_686_210,
        "31cc3a6bc638ac6c48dc4b1b74888f1eaadf3d14227c1e5f67daf9a49541d97a",
        TarGz,
        "llama-server"
    ),
    package!(
        "linux-aarch64",
        "llama-b11960-bin-ubuntu-arm64.tar.gz",
        18_411_465,
        "0065ecfbffe8a790bb0d28059a3320cdc8f723b7f3f248cdc8bfc04790927715",
        TarGz,
        "llama-server"
    ),
];

pub fn for_platform(os: &str, arch: &str) -> Option<RuntimePackage> {
    let target = format!("{os}-{arch}");
    PACKAGES
        .into_iter()
        .find(|package| package.target == target)
}
pub fn host() -> Result<RuntimePackage, String> {
    for_platform(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        "No pinned CPU runtime is available for this operating system and architecture.".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_matches_each_target_without_cross_architecture_fallback() {
        for (os, arch) in [
            ("windows", "x86_64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let p = for_platform(os, arch).unwrap();
            assert_eq!(p.target, format!("{os}-{arch}"));
            assert!(p.asset.url.starts_with(BASE));
            assert!(p.asset.url.ends_with(p.asset.filename));
            assert_eq!(p.asset.sha256.len(), 64);
            assert!(p.asset.bytes < 400 * 1024 * 1024);
        }
        for (os, arch) in [
            ("windows", "aarch64"),
            ("linux", "s390x"),
            ("freebsd", "x86_64"),
        ] {
            assert!(for_platform(os, arch).is_none());
        }
    }
}
