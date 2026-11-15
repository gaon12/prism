//! Real private-pipe regression: profiling must not change the validated wire
//! contract or hide callback errors. No SCM registration or live drive access.
use super::*;
use prism_ntfs_protocol::{
    stream::Event, Acknowledgement, DriveLetter, FileRecord, Request, Response,
};
use std::sync::atomic::AtomicU64;

const DEVICE: &str = r"\Device\PrismProfileFixture";

fn fixture(profiled: bool, callback_fails: bool, wrong_volume: bool) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        r"\\.\pipe\Prism.indexer.test.profile.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let drive = DriveLetter::new(b'C').unwrap();
    let request = Request::Snapshot { drive };
    let responses = [
        Response::Volume {
            serial: if wrong_volume { 8 } else { 7 },
            device: DEVICE.into(),
        },
        Response::Progress { completed: 1 },
        Response::Records(vec![FileRecord {
            path: r"C:\PrismProfileFixture\자료.md".into(),
            is_dir: false,
            size: 42,
            modified_ms: Some(123),
        }]),
        Response::Finished { revision: 3 },
    ];
    let mut bytes = Vec::new();
    for response in responses {
        response.write(&mut bytes, drive).unwrap();
    }
    let expected_bytes = bytes.len() as u64;
    let (ready, waiting) = std::sync::mpsc::channel();
    let server_name = name.clone();
    let thread = std::thread::spawn(move || {
        let mut server = Server::fixture(&server_name).unwrap();
        ready.send(()).unwrap();
        let mut pipe = server
            .accept(Arc::new(AtomicBool::new(false)), Duration::from_secs(3))
            .unwrap();
        assert_eq!(Request::read(&mut pipe).unwrap(), request);
        pipe.write_all(&bytes).unwrap();
        if !callback_fails && !wrong_volume {
            assert_eq!(
                Acknowledgement::read(&mut pipe).unwrap(),
                Acknowledgement::Complete { revision: 3 }
            );
        } else {
            // Keep the server alive until the client rejects and disconnects;
            // a server-side disconnect could otherwise discard unread frames.
            assert_eq!(pipe.read(&mut [0]).unwrap(), 0);
        }
    });
    waiting.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut pipe = Connection::fixture_client(&name).unwrap();
    let mut events = Vec::new();
    let mut receive = |event| {
        if let Event::Records(rows) = &event {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].path, r"C:\PrismProfileFixture\자료.md");
            assert_eq!(rows[0].size, 42);
            if callback_fails {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            std::thread::sleep(Duration::from_millis(20));
            events.push("records");
        } else {
            assert!(matches!(event, Event::Progress { completed: 1 }));
            events.push("progress");
        }
        Ok(())
    };
    let mut timing = ExchangeTiming {
        frames: 99,
        ..Default::default()
    };
    let result = if profiled {
        pipe.exchange_profiled(request, 7, DEVICE, &mut receive, &mut timing)
    } else {
        pipe.exchange(request, 7, DEVICE, &mut receive)
    };
    if wrong_volume {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(events.is_empty());
    } else if callback_fails {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(events, ["progress"]);
    } else {
        assert_eq!(result.unwrap(), 3);
        assert_eq!(events, ["progress", "records"]);
        if profiled {
            assert_eq!(timing.frames, 4);
            assert_eq!(timing.wire_bytes, expected_bytes);
            assert!(timing.read_calls >= timing.frames);
            assert!(timing.receive_callback >= Duration::from_millis(20));
            assert!(
                timing.total
                    >= timing.read
                        + timing.decode_validate
                        + timing.stream_validate
                        + timing.receive_callback
                        + timing.request_ack
            );
        }
    }
    drop(pipe);
    thread.join().unwrap();
}

#[test]
fn profiled_and_ordinary_exchange_keep_events_and_acknowledgement() {
    for profiled in [false, true] {
        fixture(profiled, false, false);
    }
}

#[test]
fn profiled_and_ordinary_exchange_propagate_callback_failure() {
    for profiled in [false, true] {
        fixture(profiled, true, false);
    }
}

#[test]
fn profiled_and_ordinary_exchange_reject_volume_mismatch_before_callback() {
    for profiled in [false, true] {
        fixture(profiled, false, true);
    }
}
