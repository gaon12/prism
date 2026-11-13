//! Explicit one-worker CPU profile of the actual codec, mapper and publication.
use super::*;
use prism_ntfs_protocol::{DriveLetter, Response, MAX_BATCH_RECORDS};
use std::io::{Cursor, Read, Write};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE},
    System::Threading::{GetCurrentThread, GetCurrentThreadId, GetThreadTimes},
};

// Keep this optional diagnostic out of the app's ordinary Windows feature set.
// Signature matches windows-sys' WindowsProgramming::QueryThreadCycleTime.
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "QueryThreadCycleTime"]
    fn query_thread_cycles(thread: HANDLE, cycles: *mut u64) -> windows_sys::core::BOOL;
}

fn file_ticks(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}
fn thread_usage() -> (u64, u64, u64) {
    let blank = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (blank, blank, blank, blank);
    let mut cycles = 0;
    // The current-thread pseudo handle needs no closing. All output pointers
    // point to initialized locals; the undefined live-thread exit time is unused.
    unsafe {
        assert_ne!(
            GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user
            ),
            0
        );
        assert_ne!(query_thread_cycles(GetCurrentThread(), &mut cycles), 0);
    }
    (file_ticks(user), file_ticks(kernel), cycles)
}

struct CpuMark {
    wall: Instant,
    user: u64,
    kernel: u64,
    cycles: u64,
}
#[derive(Default)]
struct CpuCost {
    wall: Duration,
    user: u64,
    kernel: u64,
    cycles: u64,
}
impl CpuMark {
    fn now() -> Self {
        let wall = Instant::now();
        let (user, kernel, cycles) = thread_usage();
        Self {
            wall,
            user,
            kernel,
            cycles,
        }
    }
    fn elapsed(self) -> CpuCost {
        let (user, kernel, cycles) = thread_usage();
        CpuCost {
            wall: self.wall.elapsed(),
            user: user.checked_sub(self.user).unwrap(),
            kernel: kernel.checked_sub(self.kernel).unwrap(),
            cycles: cycles.checked_sub(self.cycles).unwrap(),
        }
    }
}
impl CpuCost {
    fn add(&mut self, other: Self) {
        self.wall += other.wall;
        self.user += other.user;
        self.kernel += other.kernel;
        self.cycles += other.cycles;
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"wallMs":self.wall.as_secs_f64()*1000.0,
            "userCpuMs":self.user as f64/10_000.0,"kernelCpuMs":self.kernel as f64/10_000.0,
            "cpuMs":(self.user+self.kernel) as f64/10_000.0,"threadCycles":self.cycles})
    }
}
fn timed<T>(cost: &mut CpuCost, work: impl FnOnce() -> T) -> T {
    let mark = CpuMark::now();
    let value = work();
    cost.add(mark.elapsed());
    value
}

fn stored_checksum(entries: &Entries) -> u64 {
    let mut state = DefaultHasher::new();
    let mut buffer = String::new();
    // Stream the complete stored representation; no whole-catalog owned copy.
    for entry in entries.refs() {
        entry.write_path(&mut buffer).hash(&mut state);
        entry.write_key(&mut buffer).hash(&mut state);
        entry.has_search().hash(&mut state);
        if entry.has_search() {
            entry.write_search(&mut buffer).hash(&mut state);
        }
        entry.write_initials(&mut buffer).hash(&mut state);
        entry.write_keyboard(&mut buffer).hash(&mut state);
        entry.has_name().hash(&mut state);
        entry.is_dir().hash(&mut state);
        entry.size().hash(&mut state);
        entry.modified_ms().hash(&mut state);
        entry.seen().hash(&mut state);
    }
    state.finish()
}

#[test]
#[ignore = "Explicit gated synthetic 2.2M one-worker CPU profile; no service or GUI"]
fn synthetic_preparation_cpu_stages() {
    assert_eq!(
        std::env::var("PRISM_FILENAME_CPU_START_GATE").as_deref(),
        Ok("1")
    );
    let count = std::env::var("PRISM_FILENAME_CPU_RECORDS")
        .map_or(2_200_000, |value| value.parse::<usize>().unwrap());
    assert!((1..=2_500_000).contains(&count));
    let mode = std::env::var("PRISM_FILENAME_CPU_MODE").unwrap_or_else(|_| "stages".into());
    assert!(["fused", "stages"].contains(&mode.as_str()));
    println!("\n{{\"waitingForStart\":true}}");
    std::io::stdout().flush().unwrap();
    let mut gate = [0];
    std::io::stdin().read_exact(&mut gate).unwrap();
    assert_eq!(gate, *b"G");
    assert_eq!(super::super::super::usable_cpus(), 1);
    // SAFETY: querying the current worker's numeric identity has no side effects.
    let worker = unsafe { GetCurrentThreadId() };
    let whole = CpuMark::now();
    let (
        mut setup,
        mut calibration,
        mut input,
        mut decode,
        mut raw_cost,
        mut entry_cost,
        mut fused,
        mut insert,
        mut publish,
        mut verify,
    ) = (
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
        CpuCost::default(),
    );
    timed(&mut calibration, || {
        for _ in 0..2048 {
            std::hint::black_box(CpuMark::now().elapsed());
        }
    });
    let ((index, _owner), mut prepared, mut raws, mut ready, mut frame) = timed(&mut setup, || {
        (
            isolated(),
            Entries::new(),
            Vec::with_capacity(MAX_BATCH_RECORDS),
            Vec::with_capacity(MAX_BATCH_RECORDS),
            Vec::new(),
        )
    });
    let drive = DriveLetter::new(b'C').unwrap();
    let mut wire_bytes = 0u64;
    let mut received = 0usize;
    for start in (0..count).step_by(MAX_BATCH_RECORDS) {
        timed(&mut input, || {
            let rows: Vec<_> = (start..(start + MAX_BATCH_RECORDS).min(count))
                .map(row)
                .collect();
            frame.clear();
            Response::Records(rows).write(&mut frame, drive).unwrap();
        });
        wire_bytes += frame.len() as u64;
        let rows = timed(&mut decode, || {
            let mut cursor = Cursor::new(frame.as_slice());
            let Response::Records(rows) = Response::read(&mut cursor, drive).unwrap() else {
                panic!("record frame required")
            };
            assert_eq!(cursor.position() as usize, frame.len());
            rows
        });
        received += rows.len();
        if mode == "stages" {
            timed(&mut raw_cost, || {
                raws.extend(rows.into_iter().map(catalog::raw));
            });
            timed(&mut entry_cost, || {
                ready.extend(raws.drain(..).map(|raw| Entry::new(raw, 1)));
            });
        } else {
            timed(&mut fused, || {
                ready.extend(rows.into_iter().map(|row| Entry::new(catalog::raw(row), 1)));
            });
        }
        timed(&mut insert, || {
            for entry in ready.drain(..) {
                catalog::insert(&mut prepared, entry);
            }
        });
    }
    // Finish uses the actual terminal codec too; no wire I/O is simulated.
    timed(&mut input, || {
        frame.clear();
        Response::Finished { revision: 1 }
            .write(&mut frame, drive)
            .unwrap();
    });
    wire_bytes += frame.len() as u64;
    timed(&mut decode, || {
        assert!(matches!(
            Response::read(&mut Cursor::new(&frame), drive).unwrap(),
            Response::Finished { revision: 1 }
        ));
    });
    assert_eq!(received, count);
    assert_eq!(prepared.len(), count);
    let before = timed(&mut verify, || stored_checksum(&prepared));
    let shared = timed(&mut setup, || {
        let shared = Arc::new(Mutex::new(prepared));
        index.show_provisional(1, &shared);
        assert!(index.catalog.read().unwrap().provisional.is_some());
        assert!(index.catalog.read().unwrap().entries.is_empty());
        shared
    });
    timed(&mut publish, || {
        index.publish_provisional(1, &shared).unwrap()
    });
    let after = timed(&mut verify, || {
        let catalog = index.catalog.read().unwrap();
        assert!(catalog.provisional.is_none());
        assert_eq!(catalog.status.count, count);
        assert_eq!(catalog.entries.len(), count);
        assert!(shared.lock().unwrap().is_empty());
        stored_checksum(&catalog.entries)
    });
    assert_eq!(
        before, after,
        "real publication must preserve all stored fields"
    );
    // SAFETY: as above; the entire payload stayed on this single worker.
    assert_eq!(unsafe { GetCurrentThreadId() }, worker);
    let whole = whole.elapsed();
    println!(
        "{}",
        serde_json::json!({"synthetic":true,"records":count,"workers":1,
        "mode":mode,"wireBytes":wire_bytes,"workerThreadId":worker,
        "cpuClock":"GetThreadTimes user+kernel; raw quantized diagnostics, not additive phase attribution",
        "cycleClock":"QueryThreadCycleTime; cycles are not converted to time",
        "inputPreparation":input.json(),"decode":decode.json(),"raw":raw_cost.json(),
        "entry":entry_cost.json(),"fusedRawEntry":fused.json(),"insert":insert.json(),
        "publishSwap":publish.json(),"setup":setup.json(),"clockCalibration2048":calibration.json(),
        "verification":verify.json(),"wholeWorker":whole.json(),
        "storedChecksumBefore":format!("{before:016x}"),"storedChecksumAfter":format!("{after:016x}"),
        "realProvisionalSwap":true,"wireIo":false,"gui":false})
    );
}
