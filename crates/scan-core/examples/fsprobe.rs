//! Does FSEvents answer "what changed under this path since event id N"?
//!
//! Everything an incremental rescan needs rests on that one question, and the
//! answer is not in the documentation in a form worth trusting: the history
//! lives in `/.fseventsd` on the volume, it is rotated, and it is reported
//! lost through flags rather than through an error. So this asks the real
//! thing before any of it is designed around.
//!
//! ```text
//! cargo run --release -p spacetrace-scan-core --example fsprobe -- <dir>
//! ```
//!
//! It prints the current event id, makes a handful of changes under `<dir>`,
//! and then replays from the id it started with. `replay <id>` asks the same
//! question with an id you supply, and times it.
//!
//! Measured 22 September 2026 under `~/Desktop/Projects`, against a full scan
//! of the same tree at 1431 ms: 1,000 ids back 7.7 ms, 10,000 back 10.2 ms,
//! 100,000 back 29.9 ms, 1,000,000 back 835 ms, and the whole history 19.4 s.
//! The last one is the shape of the failure — far past the point where walking
//! everything is cheaper, and reported by not finishing rather than by a flag.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}

#[cfg(target_os = "macos")]
fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| usage()));
    let mode = args.next();

    match mode.as_deref() {
        // `replay <id> [seconds]` — what an incremental rescan would ask, with
        // the cursor a previous scan would have stored. The timing is the
        // point: the cost grows with how far back the id is, not with how much
        // changed, and past some distance a full rescan is simply cheaper.
        Some("replay") => {
            let id: u64 = args
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| usage());
            let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
            let started = std::time::Instant::now();
            let out = fsevents::since(&root, id, std::time::Duration::from_secs(secs));
            println!("replay from {id} took {:?}", started.elapsed());
            report(out);
        }
        // No mode: make three shapes of change and replay across them, which
        // is the check that the history is being read at all.
        None => {
            std::fs::create_dir_all(&root).unwrap();
            let before = fsevents::current_event_id();
            println!("event id before: {before}");

            // A new file in a new directory, a file grown **in place** — which
            // changes no directory's mtime, and is the case that rules out
            // comparing mtimes instead of asking — and a deletion.
            std::fs::create_dir_all(root.join("fresh/deeper")).unwrap();
            std::fs::write(root.join("fresh/deeper/new.bin"), vec![1u8; 4096]).unwrap();
            std::fs::write(root.join("grown.bin"), vec![2u8; 1024]).unwrap();
            std::fs::write(root.join("grown.bin"), vec![2u8; 200_000]).unwrap();
            std::fs::write(root.join("doomed.bin"), b"x").unwrap();
            std::fs::remove_file(root.join("doomed.bin")).unwrap();

            report(fsevents::since(
                &root,
                before,
                std::time::Duration::from_secs(30),
            ));
            println!("event id after: {}", fsevents::current_event_id());
        }
        Some(other) => {
            eprintln!("unknown mode {other:?}");
            usage()
        }
    }
}

#[cfg(target_os = "macos")]
fn usage() -> ! {
    eprintln!("usage: fsprobe <dir> [replay <event-id> [seconds]]");
    std::process::exit(2)
}

#[cfg(target_os = "macos")]
fn report(out: Result<Vec<(std::path::PathBuf, u32)>, String>) {
    match out {
        Ok(list) => {
            println!("{} events", list.len());
            for (path, flags) in list.iter().take(20) {
                println!("  {flags:#08x}  {}", path.display());
            }
            if list.len() > 20 {
                println!("  … {} more", list.len() - 20);
            }
        }
        // Not an error so much as the answer "ask someone else": the caller's
        // fallback is a full walk, which is what was correct anyway.
        Err(e) => println!("no answer: {e}"),
    }
}

#[cfg(target_os = "macos")]
mod fsevents {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    pub type EventId = u64;
    type Flags = u32;

    const HISTORY_DONE: Flags = 0x0000_0010;
    const FILE_EVENTS: u32 = 0x0000_0010;
    const UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    struct Context {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    type Callback =
        extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const Flags, *const EventId);

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn FSEventsGetCurrentEventId() -> EventId;
        fn FSEventStreamCreate(
            allocator: *const c_void,
            callback: Callback,
            context: *const Context,
            paths: *const c_void,
            since: EventId,
            latency: f64,
            flags: u32,
        ) -> *mut c_void;
        fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *mut c_void);
        fn FSEventStreamStart(stream: *mut c_void) -> u8;
        fn FSEventStreamStop(stream: *mut c_void);
        fn FSEventStreamInvalidate(stream: *mut c_void);
        fn FSEventStreamRelease(stream: *mut c_void);

        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: u8,
        ) -> *const c_void;
        fn CFArrayCreate(
            allocator: *const c_void,
            values: *const *const c_void,
            count: isize,
            callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
        static kCFTypeArrayCallBacks: c_void;
    }

    extern "C" {
        fn dispatch_queue_create(label: *const i8, attr: *const c_void) -> *mut c_void;
        fn dispatch_release(object: *mut c_void);
    }

    pub fn current_event_id() -> EventId {
        unsafe { FSEventsGetCurrentEventId() }
    }

    struct Sink {
        events: Mutex<(Vec<(PathBuf, Flags)>, bool)>,
        done: Condvar,
    }

    extern "C" fn on_events(
        _stream: *const c_void,
        info: *mut c_void,
        count: usize,
        paths: *mut c_void,
        flags: *const Flags,
        _ids: *const EventId,
    ) {
        let sink = unsafe { &*(info as *const Sink) };
        let paths = paths as *const *const i8;
        let mut guard = sink.events.lock().unwrap();
        for i in 0..count {
            let flag = unsafe { *flags.add(i) };
            if flag & HISTORY_DONE != 0 {
                guard.1 = true;
                continue;
            }
            let raw = unsafe { std::ffi::CStr::from_ptr(*paths.add(i)) };
            use std::os::unix::ffi::OsStrExt;
            let path = PathBuf::from(std::ffi::OsStr::from_bytes(raw.to_bytes()));
            guard.0.push((path, flag));
        }
        if guard.1 {
            sink.done.notify_all();
        }
    }

    pub fn since(
        root: &Path,
        since: EventId,
        patience: Duration,
    ) -> Result<Vec<(PathBuf, Flags)>, String> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = root.as_os_str().as_bytes();

        let sink = Arc::new(Sink {
            events: Mutex::new((Vec::new(), false)),
            done: Condvar::new(),
        });

        unsafe {
            let cf_path = CFStringCreateWithBytes(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len() as isize,
                UTF8,
                0,
            );
            if cf_path.is_null() {
                return Err("CFStringCreateWithBytes failed".into());
            }
            let values = [cf_path];
            let array = CFArrayCreate(
                std::ptr::null(),
                values.as_ptr(),
                1,
                &kCFTypeArrayCallBacks as *const c_void,
            );
            CFRelease(cf_path);
            if array.is_null() {
                return Err("CFArrayCreate failed".into());
            }

            let context = Context {
                version: 0,
                info: Arc::as_ptr(&sink) as *mut c_void,
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            let stream = FSEventStreamCreate(
                std::ptr::null(),
                on_events,
                &context,
                array,
                since,
                0.0,
                FILE_EVENTS,
            );
            CFRelease(array);
            if stream.is_null() {
                return Err("FSEventStreamCreate failed".into());
            }

            let label = c"spacetrace.fsprobe";
            let queue = dispatch_queue_create(label.as_ptr(), std::ptr::null());
            FSEventStreamSetDispatchQueue(stream, queue);
            if FSEventStreamStart(stream) == 0 {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
                dispatch_release(queue);
                return Err("FSEventStreamStart failed".into());
            }

            let mut guard = sink.events.lock().unwrap();
            while !guard.1 {
                let (g, timeout) = sink.done.wait_timeout(guard, patience).unwrap();
                guard = g;
                if timeout.timed_out() {
                    break;
                }
            }
            let out = guard.0.clone();
            let saw_history_done = guard.1;
            drop(guard);

            FSEventStreamStop(stream);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            dispatch_release(queue);

            if !saw_history_done {
                return Err("no HistoryDone within the deadline".into());
            }
            Ok(out)
        }
    }
}
