//! The bridge between Ravune and its plugin host processes (M7).
//!
//! Every plugin runs in its own `ravune-host` process, so a crash takes down that plugin
//! only. Per audio block the engine writes events (with frame offsets) and input audio into
//! a shared-memory block, bumps a request counter and signals; the host processes and
//! answers by copying the counter to `done`. If the answer doesn't come in time the block
//! is silent and the plugin is marked late; while it's still busy no new block is sent.
//! Control (load, state, parameters, the editor) is JSON lines over the host's stdin/stdout.
//!
//! Signaling uses named events on Windows and named pipes on macOS and Linux: both sides
//! sleep until the other one has something.

pub mod control;

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};

pub use control::{Command, Format, Found, Hosted, ParamInfo, Reply, decode, encode, scan};
use memmap2::MmapMut;

/// The most frames one block can carry (the engine sends 256).
pub const MAX_FRAMES: usize = 2048;
pub const MAX_EVENTS: usize = 1024;
const MAGIC: u32 = 0x5048_4231; // "PHB1"

/// One event for the plugin, `frame` frames into the block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Event {
    pub frame: u32,
    pub kind: u32,
    /// Note key, parameter id or CC number.
    pub key: u32,
    /// MIDI channel (MPE: one per note).
    pub channel: u32,
    /// Velocity 0..1, parameter value, pitch bend -1..1, pressure or CC 0..1.
    pub value: f64,
}

pub mod kind {
    pub const NOTE_ON: u32 = 1;
    pub const NOTE_OFF: u32 = 2;
    pub const PARAM: u32 = 3;
    pub const BEND: u32 = 4;
    pub const PRESSURE: u32 = 5;
    pub const CC: u32 = 6;
    /// Every note off (transport stopped, the plugin came back from being late).
    pub const ALL_OFF: u32 = 7;
}

/// The shared block. Both processes map the same file; `request` / `done` (release /
/// acquire) order every other access.
#[repr(C)]
struct Shared {
    magic: u32,
    request: AtomicU32,
    done: AtomicU32,
    frames: AtomicU32,
    events: AtomicU32,
    playing: AtomicU32,
    tempo: AtomicU64,
    beats: AtomicU64,
    event: [Event; MAX_EVENTS],
    input: [[f32; MAX_FRAMES]; 2],
    output: [[f32; MAX_FRAMES]; 2],
}

fn map(path: &Path, create: bool) -> io::Result<(MmapMut, *mut Shared)> {
    let file = OpenOptions::new().read(true).write(true).create(create).truncate(create).open(path)?;
    if create {
        file.set_len(size_of::<Shared>() as u64)?;
    }
    // SAFETY: the file is ours (a fresh temp file shared with our own host process only).
    let mut map = unsafe { MmapMut::map_mut(&file)? };
    if map.len() < size_of::<Shared>() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "shared block too small"));
    }
    let shared = map.as_mut_ptr().cast::<Shared>();
    Ok((map, shared))
}

/// The signal names for a shared-memory file (both sides derive them the same way).
fn names(path: &Path) -> (String, String) {
    let base = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    (format!("{base}-req"), format!("{base}-done"))
}

// ---------------------------------------------------------------------------
// Client: the engine's side, called on the audio thread
// ---------------------------------------------------------------------------

/// One plugin as the audio thread sees it.
pub struct Bridge {
    _map: MmapMut,
    shared: *mut Shared,
    request: signal::Signal,
    done: signal::Signal,
    dead: AtomicBool,
    late: AtomicU32,
    /// The last block went unanswered: tell the plugin to let go of its notes.
    resync: AtomicBool,
}

// SAFETY: the mapping lives as long as the Bridge; the audio thread is the only writer of
// the client half, and the counters order everything else.
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

impl Bridge {
    /// Creates the shared block at `path` (the host process opens it with [`Server::open`]).
    pub fn create(path: &Path) -> io::Result<Self> {
        let (map, shared) = map(path, true)?;
        // SAFETY: freshly mapped and zeroed; nobody else has it yet.
        unsafe { (*shared).magic = MAGIC };
        let (req, done) = names(path);
        Ok(Self { _map: map, shared, request: signal::Signal::create(&req)?, done: signal::Signal::create(&done)?, dead: AtomicBool::new(false), late: AtomicU32::new(0), resync: AtomicBool::new(false) })
    }

    fn shared(&self) -> &Shared {
        // SAFETY: valid for our lifetime; fields we read concurrently are atomics.
        unsafe { &*self.shared }
    }

    fn idle(&self) -> bool {
        let s = self.shared();
        s.done.load(Acquire) == s.request.load(Relaxed)
    }

    /// Queues an event for the next block. Dropped while the plugin is still busy.
    pub fn event(&self, e: Event) {
        if !self.idle() || self.dead.load(Relaxed) {
            return;
        }
        let s = self.shared();
        let n = s.events.load(Relaxed) as usize;
        if n < MAX_EVENTS {
            // SAFETY: idle, so the host isn't reading the events; we're their only writer.
            unsafe { (*self.shared).event[n] = e };
            s.events.store(n as u32 + 1, Relaxed);
        }
    }

    /// One round trip: `input` (effects; `None` for instruments) through the plugin into
    /// `out`. Returns false, with `out` silent, if the plugin is busy, late or gone.
    pub fn process(&self, input: Option<(&[f32], &[f32])>, out: (&mut [f32], &mut [f32]), tempo: f64, beats: f64, playing: bool, timeout: Duration) -> bool {
        let (out_l, out_r) = out;
        let frames = out_l.len().min(out_r.len()).min(MAX_FRAMES);
        out_l.fill(0.0);
        out_r.fill(0.0);
        if self.dead.load(Relaxed) || !self.idle() {
            return false;
        }
        let s = self.shared();
        if self.resync.swap(false, Relaxed) {
            self.event(Event { kind: kind::ALL_OFF, ..Default::default() });
        }
        // SAFETY: idle, so the host doesn't touch the input until we publish the request.
        unsafe {
            let shared = &mut *self.shared;
            match input {
                Some((l, r)) => {
                    shared.input[0][..frames].copy_from_slice(&l[..frames]);
                    shared.input[1][..frames].copy_from_slice(&r[..frames]);
                }
                None => {
                    shared.input[0][..frames].fill(0.0);
                    shared.input[1][..frames].fill(0.0);
                }
            }
        }
        s.frames.store(frames as u32, Relaxed);
        s.tempo.store(tempo.to_bits(), Relaxed);
        s.beats.store(beats.to_bits(), Relaxed);
        s.playing.store(playing as u32, Relaxed);
        let id = s.request.load(Relaxed).wrapping_add(1);
        s.request.store(id, Release);
        self.request.set();
        let start = Instant::now();
        while s.done.load(Acquire) != id {
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                self.late.fetch_add(1, Relaxed);
                self.resync.store(true, Relaxed);
                return false;
            }
            self.done.wait(left);
        }
        // SAFETY: done == id: the host finished writing the output and won't touch it again.
        unsafe {
            let shared = &*self.shared;
            out_l[..frames].copy_from_slice(&shared.output[0][..frames]);
            out_r[..frames].copy_from_slice(&shared.output[1][..frames]);
        }
        s.events.store(0, Relaxed);
        true
    }

    /// The host process is gone: stop waiting for it.
    pub fn kill(&self) {
        self.dead.store(true, Relaxed);
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Relaxed)
    }

    /// Blocks that came back too late (or never).
    pub fn late(&self) -> u32 {
        self.late.load(Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Server: the host process's side
// ---------------------------------------------------------------------------

/// One block to process, borrowed from the shared memory.
pub struct Block<'a> {
    pub frames: usize,
    pub events: &'a [Event],
    pub input: [&'a [f32]; 2],
    pub output: [&'a mut [f32]; 2],
    pub tempo: f64,
    pub beats: f64,
    pub playing: bool,
}

pub struct Server {
    _map: MmapMut,
    shared: *mut Shared,
    request: signal::Signal,
    done: signal::Signal,
    seen: u32,
}

// SAFETY: as for Bridge; the host's audio thread owns the Server.
unsafe impl Send for Server {}

impl Server {
    pub fn open(path: &Path) -> io::Result<Self> {
        let (map, shared) = map(path, false)?;
        // SAFETY: mapped; magic is written once by the creator before the host starts.
        if unsafe { (*shared).magic } != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Ravune bridge"));
        }
        let (req, done) = names(path);
        // SAFETY: as above.
        let seen = unsafe { (*shared).done.load(Acquire) };
        Ok(Self { _map: map, shared, request: signal::Signal::open(&req)?, done: signal::Signal::open(&done)?, seen })
    }

    /// Waits up to `timeout` for a block; the caller processes it and calls [`Self::finish`].
    pub fn wait(&mut self, timeout: Duration) -> Option<Block<'_>> {
        // SAFETY: valid mapping; counters are atomics.
        let s = unsafe { &*self.shared };
        let start = Instant::now();
        while s.request.load(Acquire) == self.seen {
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return None;
            }
            self.request.wait(left);
        }
        self.seen = s.request.load(Acquire);
        let frames = (s.frames.load(Relaxed) as usize).min(MAX_FRAMES);
        let events = (s.events.load(Relaxed) as usize).min(MAX_EVENTS);
        // SAFETY: a request is published and not yet answered: the client keeps off until `done`.
        let shared = unsafe { &mut *self.shared };
        let [in_l, in_r] = &shared.input;
        let [out_l, out_r] = &mut shared.output;
        Some(Block {
            frames,
            events: &shared.event[..events],
            input: [&in_l[..frames], &in_r[..frames]],
            output: [&mut out_l[..frames], &mut out_r[..frames]],
            tempo: f64::from_bits(s.tempo.load(Relaxed)),
            beats: f64::from_bits(s.beats.load(Relaxed)),
            playing: s.playing.load(Relaxed) != 0,
        })
    }

    /// The block's output is written: hand it back.
    pub fn finish(&mut self) {
        // SAFETY: valid mapping.
        let s = unsafe { &*self.shared };
        s.done.store(self.seen, Release);
        self.done.set();
    }
}

#[cfg(windows)]
mod signal {
    use std::io;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject};

    /// The standard right to wait on an object.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    /// A named auto-reset event.
    pub struct Signal(HANDLE);

    // SAFETY: event handles may be used from any thread.
    unsafe impl Send for Signal {}
    unsafe impl Sync for Signal {}

    fn wide(name: &str) -> Vec<u16> {
        format!("Local\\{name}").encode_utf16().chain(Some(0)).collect()
    }

    impl Signal {
        pub fn create(name: &str) -> io::Result<Self> {
            // SAFETY: valid null-terminated name; default security; auto-reset, unset.
            let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, wide(name).as_ptr()) };
            if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self(h)) }
        }

        pub fn open(name: &str) -> io::Result<Self> {
            // SAFETY: valid null-terminated name.
            let h = unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, wide(name).as_ptr()) };
            if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self(h)) }
        }

        pub fn set(&self) {
            // SAFETY: a valid event handle.
            unsafe { SetEvent(self.0) };
        }

        /// True if signaled within `timeout`.
        pub fn wait(&self, timeout: Duration) -> bool {
            let ms = timeout.as_millis().clamp(1, u32::MAX as u128 - 1) as u32;
            // SAFETY: a valid event handle.
            unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Signal {
        fn drop(&mut self) {
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(unix)]
mod signal {
    use std::ffi::CString;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// A named pipe in the temp folder, open in both processes: `set` writes a byte, `wait`
    /// sleeps until one comes and drains them all (an auto-reset event).
    pub struct Signal {
        pipe: File,
        /// Ours to remove: this side made it.
        path: Option<PathBuf>,
    }

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}.fifo"))
    }

    /// Both ends and no blocking, so neither side waits for the other to open it.
    fn fifo(path: &Path) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path)
    }

    impl Signal {
        pub fn create(name: &str) -> io::Result<Self> {
            let path = path(name);
            let _ = std::fs::remove_file(&path); // left by a crash
            let c = CString::new(path.as_os_str().as_bytes())?;
            // SAFETY: a valid null-terminated path.
            if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { pipe: fifo(&path)?, path: Some(path) })
        }

        pub fn open(name: &str) -> io::Result<Self> {
            Ok(Self { pipe: fifo(&path(name))?, path: None })
        }

        pub fn set(&self) {
            let _ = (&self.pipe).write(&[1]); // a full pipe is signaled already
        }

        /// True if signaled within `timeout`.
        pub fn wait(&self, timeout: Duration) -> bool {
            let mut p = libc::pollfd { fd: self.pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ms = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
            // SAFETY: one valid pollfd.
            if unsafe { libc::poll(&mut p, 1, ms) } <= 0 {
                return false;
            }
            let mut drain = [0u8; 64];
            while matches!((&self.pipe).read(&mut drain), Ok(n) if n > 0) {}
            true
        }
    }

    impl Drop for Signal {
        fn drop(&mut self) {
            if let Some(path) = &self.path {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_and_late_hosts_go_silent() {
        let path = std::env::temp_dir().join(format!("ravune-bridge-test-{}.shm", std::process::id()));
        let bridge = Bridge::create(&path).unwrap();
        let mut server = Server::open(&path).unwrap();
        let host = std::thread::spawn(move || {
            // an "effect" that doubles its input and adds one per note-on
            for _ in 0..2 {
                let Some(block) = server.wait(Duration::from_secs(5)) else { return };
                let notes = block.events.iter().filter(|e| e.kind == kind::NOTE_ON).count() as f32;
                let [l, r] = block.output;
                for i in 0..block.frames {
                    l[i] = block.input[0][i] * 2.0 + notes;
                    r[i] = block.input[1][i] * 2.0 + block.tempo as f32;
                }
                server.finish();
            }
            // third block: never answered
            let _ = server.wait(Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(300));
        });
        let (input_l, input_r) = (vec![0.25f32; 256], vec![0.5f32; 256]);
        let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        bridge.event(Event { kind: kind::NOTE_ON, key: 60, value: 1.0, ..Default::default() });
        assert!(bridge.process(Some((&input_l, &input_r)), (&mut l, &mut r), 120.0, 0.0, true, Duration::from_secs(5)));
        assert_eq!((l[0], r[255]), (1.5, 121.0));
        assert!(bridge.process(Some((&input_l, &input_r)), (&mut l, &mut r), 90.0, 0.0, true, Duration::from_secs(5)));
        assert_eq!((l[0], r[0]), (0.5, 91.0), "events are used once");
        assert!(!bridge.process(None, (&mut l, &mut r), 90.0, 0.0, true, Duration::from_millis(50)), "late");
        assert_eq!((bridge.late(), l[0]), (1, 0.0));
        assert!(!bridge.process(None, (&mut l, &mut r), 90.0, 0.0, true, Duration::from_millis(50)), "still busy: nothing sent");
        bridge.kill();
        assert!(!bridge.process(None, (&mut l, &mut r), 90.0, 0.0, true, Duration::from_secs(5)), "dead returns at once");
        host.join().unwrap();
        drop(bridge);
        let _ = std::fs::remove_file(path);
    }
}
