//! Conservative, request-local disk admission for parallel content reads.
//! This is scheduling evidence, never source authority. A failed/unknown probe
//! permits only serial work. No device names or source paths are logged.
use std::{
    collections::HashMap,
    hash::Hash,
    path::{Path, PathBuf},
};
const MAX_DEVICES: usize = 128;

struct Cache<K> {
    devices: HashMap<K, bool>,
}
impl<K> Default for Cache<K> {
    fn default() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }
}
impl<K: Eq + Hash> Cache<K> {
    fn admits(&mut self, key: Option<K>, probe: impl FnOnce(&K) -> bool) -> bool {
        let Some(key) = key else {
            return false;
        };
        if let Some(known) = self.devices.get(&key) {
            return *known;
        }
        if self.devices.len() >= MAX_DEVICES {
            return false;
        }
        let nonseek = probe(&key);
        self.devices.insert(key, nonseek);
        nonseek
    }
}
#[derive(Default)]
pub(crate) struct ReadPolicy {
    cache: Cache<platform::Device>,
}
impl ReadPolicy {
    pub(crate) fn all_roots_nonseek(&mut self, roots: &[PathBuf]) -> bool {
        !roots.is_empty() && roots.iter().all(|root| self.permits_parallel(root))
    }
    pub(crate) fn permits_parallel(&mut self, path: &Path) -> bool {
        // Resolve the actual mounted device for every path. A selected SSD root
        // does not authorize parallel reads from a nested HDD/network mount.
        self.cache.admits(platform::device(path), platform::nonseek)
    }
}

#[cfg(any(windows, test))]
fn descriptor_nonseek(bytes: &[u8], returned: usize) -> bool {
    const MINIMUM: usize = 12; // C layout: DWORD, DWORD, BOOLEAN, alignment padding.
    if returned < MINIMUM || returned > bytes.len() {
        return false;
    }
    let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    version >= MINIMUM && size >= version && size <= returned && bytes[8] == 0
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            Ioctl::{
                PropertyStandardQuery, StorageDeviceSeekPenaltyProperty,
                IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
            },
            IO::DeviceIoControl,
        },
    };
    #[derive(PartialEq, Eq, Hash)]
    pub(super) struct Device(String);
    pub(super) fn device(path: &Path) -> Option<Device> {
        std::fs::metadata(path).ok()?;
        let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
        if name.is_empty() || name.len() >= 32768 || name.contains(&0) {
            return None;
        }
        name.push(0);
        let mut mount = vec![0u16; 32768];
        let mut volume = [0u16; 128];
        // SAFETY: terminated input, initialized output buffers and exact capacities.
        unsafe {
            if GetVolumePathNameW(name.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) == 0
                || GetVolumeNameForVolumeMountPointW(
                    mount.as_ptr(),
                    volume.as_mut_ptr(),
                    volume.len() as u32,
                ) == 0
            {
                return None;
            }
        }
        let end = volume.iter().position(|v| *v == 0)?;
        let volume = String::from_utf16(&volume[..end]).ok()?;
        if !volume.starts_with(r"\\?\Volume{") || !volume.ends_with("}\\") {
            return None;
        }
        Some(Device(volume))
    }
    pub(super) fn nonseek(volume: &Device) -> bool {
        let name: Vec<u16> = volume
            .0
            .trim_end_matches('\\')
            .encode_utf16()
            .chain(Some(0))
            .collect();
        struct Owned(HANDLE);
        impl Drop for Owned {
            fn drop(&mut self) {
                // SAFETY: exclusively owned successful CreateFileW handle.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        // Request no data access or writes. No elevation/retry on denied devices.
        // SAFETY: terminated volume-device path; null security/template handles.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return false;
        }
        let owned = Owned(handle);
        let mut query = [0u8; std::mem::size_of::<STORAGE_PROPERTY_QUERY>()];
        query[0..4].copy_from_slice(&StorageDeviceSeekPenaltyProperty.to_le_bytes());
        query[4..8].copy_from_slice(&PropertyStandardQuery.to_le_bytes());
        // Receive raw bytes so malformed BOOLEAN values never construct an invalid
        // Rust bool. Only an explicit complete non-seek descriptor is accepted.
        let mut descriptor = [0u8; 64];
        let mut returned = 0u32;
        // SAFETY: owned synchronous device, initialized input/output and lengths;
        // no overlapped request or writable source-file handle is supplied.
        let ok = unsafe {
            DeviceIoControl(
                owned.0,
                IOCTL_STORAGE_QUERY_PROPERTY,
                query.as_ptr().cast::<c_void>(),
                std::mem::size_of_val(&query) as u32,
                descriptor.as_mut_ptr().cast::<c_void>(),
                descriptor.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        ok != 0 && descriptor_nonseek(&descriptor, returned as usize)
    }
}

#[cfg(any(target_os = "linux", test))]
fn stack_nonseek<K: Clone + Eq + Hash>(
    root: K,
    mut read: impl FnMut(&K) -> Option<(bool, Vec<K>)>,
) -> bool {
    let mut states = HashMap::new(); // visiting=1, complete=2; shared leaves are valid.
    let mut pending = vec![(root, false)];
    let mut steps = 0usize;
    while let Some((node, exit)) = pending.pop() {
        steps += 1;
        if steps > 1024 || pending.len() > 512 {
            return false;
        }
        if exit {
            states.insert(node, 2);
            continue;
        }
        match states.get(&node) {
            Some(1) => return false,
            Some(2) => continue,
            _ => {}
        }
        if states.len() >= MAX_DEVICES {
            return false;
        }
        let Some((nonseek, children)) = read(&node) else {
            return false;
        };
        if !nonseek || children.len() > MAX_DEVICES {
            return false;
        }
        states.insert(node.clone(), 1);
        pending.push((node, true));
        pending.extend(children.into_iter().rev().map(|child| (child, false)));
    }
    true
}
#[cfg(any(target_os = "linux", test))]
fn rotational(bytes: &[u8]) -> Option<bool> {
    if bytes.len() > 8 {
        return None;
    }
    match std::str::from_utf8(bytes).ok()?.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{fs, io::Read, os::unix::fs::MetadataExt};
    pub(super) type Device = u64;
    pub(super) fn device(path: &Path) -> Option<Device> {
        fs::metadata(path).ok().map(|metadata| metadata.dev())
    }
    fn queue_device(path: &Path, root: &Path) -> Option<PathBuf> {
        let mut path = fs::canonicalize(path).ok()?;
        // A partition has no queue directory. Walk to its parent block device,
        // never outside the canonical sysfs devices tree.
        for _ in 0..16 {
            if !path.starts_with(root) || path == root {
                return None;
            }
            if path.join("queue").is_dir() {
                return Some(path);
            }
            path = path.parent()?.to_path_buf();
        }
        None
    }
    pub(super) fn nonseek(device: &Device) -> bool {
        let root = Path::new("/sys/devices");
        let path = PathBuf::from(format!(
            "/sys/dev/block/{}:{}",
            libc::major(*device),
            libc::minor(*device)
        ));
        let Some(initial) = queue_device(&path, root) else {
            return false;
        };
        stack_nonseek(initial, |path| {
            let mut bytes = Vec::new();
            fs::File::open(path.join("queue/rotational"))
                .ok()?
                .take(9)
                .read_to_end(&mut bytes)
                .ok()?;
            let nonseek = !rotational(&bytes)?;
            let mut children = Vec::new();
            for child in fs::read_dir(path.join("slaves")).ok()? {
                if children.len() >= MAX_DEVICES {
                    return None;
                }
                children.push(queue_device(&child.ok()?.path(), root)?);
            }
            // A loop/file-backed virtual leaf can itself sit on an HDD even if
            // its own queue says nonrotational. An unknown backing store is serial.
            if children.is_empty() && path.starts_with(root.join("virtual")) {
                return None;
            }
            Some((nonseek, children))
        })
    }
}
/// macOS has no simple per-volume seek-penalty query, and current Macs boot from
/// internal SSDs. Treat each mounted device as non-seek so Mac users get parallel
/// reads; the device id still keys the cache per mount. External HDD detection
/// (IOKit) is a documented follow-up.
#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    pub(super) type Device = u64;
    pub(super) fn device(path: &Path) -> Option<Device> {
        std::fs::metadata(path).ok().map(|meta| meta.dev())
    }
    pub(super) fn nonseek(_: &Device) -> bool {
        true
    }
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;
    pub(super) type Device = ();
    pub(super) fn device(_: &Path) -> Option<Device> {
        None
    }
    pub(super) fn nonseek(_: &Device) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_cache_does_not_reuse_the_root_decision_for_a_nested_device() {
        let mut cache = Cache::default();
        let mut calls = 0;
        for (id, expected) in [(1, true), (2, false), (1, true), (2, false)] {
            assert_eq!(
                cache.admits(Some(id), |id| {
                    calls += 1;
                    *id == 1
                }),
                expected
            );
        }
        assert_eq!(calls, 2);
        assert!(!cache.admits(None, |_| panic!("unknown device must not be probed")));
    }
    #[test]
    fn bounded_device_cache_treats_new_overflow_devices_as_unknown() {
        let mut cache = Cache::default();
        for id in 0..MAX_DEVICES {
            assert!(cache.admits(Some(id), |_| true));
        }
        assert!(!cache.admits(Some(MAX_DEVICES), |_| panic!("cache bound")));
        assert!(cache.admits(Some(0), |_| panic!("cached")));
    }
    #[test]
    fn only_a_complete_explicit_nonseek_descriptor_admits_parallelism() {
        let mut bytes = [0; 64];
        bytes[0..4].copy_from_slice(&12u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&12u32.to_le_bytes());
        assert!(descriptor_nonseek(&bytes, 12));
        assert!(!descriptor_nonseek(&bytes, 8));
        assert!(!descriptor_nonseek(&bytes[..8], 12));
        bytes[8] = 1;
        assert!(!descriptor_nonseek(&bytes, 12));
        bytes[8] = 2;
        assert!(!descriptor_nonseek(&bytes, 12));
        bytes[8] = 0;
        bytes[4..8].copy_from_slice(&32u32.to_le_bytes());
        assert!(!descriptor_nonseek(&bytes, 12));
        assert!(descriptor_nonseek(&bytes, 32));
        bytes[0..4].copy_from_slice(&8u32.to_le_bytes());
        assert!(!descriptor_nonseek(&bytes, 32));
    }
    #[test]
    fn rotational_file_requires_an_exact_known_value() {
        assert_eq!(rotational(b"0\n"), Some(false));
        assert_eq!(rotational(b"1\n"), Some(true));
        for value in [b"".as_slice(), b"2", b"0 1", b"0\0", b"\xff", b"0         "] {
            assert_eq!(rotational(value), None);
        }
    }
    #[test]
    fn stacked_device_requires_all_leaves_and_rejects_cycles_or_missing_data() {
        let graph = HashMap::from([
            (0, (true, vec![1, 2])),
            (1, (true, vec![3])),
            (2, (true, vec![3])),
            (3, (true, vec![])),
        ]);
        assert!(stack_nonseek(0, |key| graph.get(key).cloned()));
        let mut mixed = graph.clone();
        mixed.insert(3, (false, vec![]));
        assert!(!stack_nonseek(0, |key| mixed.get(key).cloned()));
        assert!(!stack_nonseek(0, |key| if *key == 3 {
            None
        } else {
            graph.get(key).cloned()
        }));
        let mut cycle = graph;
        cycle.insert(3, (true, vec![0]));
        assert!(!stack_nonseek(0, |key| cycle.get(key).cloned()));
        assert!(!stack_nonseek(0, |key| Some((true, vec![*key + 1]))));
    }
    #[test]
    fn missing_path_and_empty_root_set_are_conservatively_serial() {
        let mut policy = ReadPolicy::default();
        assert!(!policy.all_roots_nonseek(&[]));
        let directory = tempfile::tempdir().unwrap();
        assert!(!policy.permits_parallel(&directory.path().join("does-not-exist/source.txt")));
    }
}
