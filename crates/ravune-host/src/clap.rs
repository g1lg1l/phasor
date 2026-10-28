//! CLAP plugins through clack-host: scanning, loading, processing (notes, MIDI for bends,
//! pressure and CCs, parameters, transport), parameters, state and the editor (embedded in
//! our window, or floating when that's all the plugin offers).

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui, Window};
#[cfg(unix)]
use clack_extensions::posix_fd::{FdFlags, HostPosixFd, HostPosixFdImpl, PluginPosixFd};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{NoteDialect, NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer, ParamInfoFlags, ParamRescanFlags, PluginParams};
use clack_extensions::state::{HostState, HostStateImpl, PluginState};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::events::event_types::{Midi2Event, MidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent, TransportEvent, TransportFlags};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{EventFlags, Match};
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use ravune_bridge::{Block, Format, Found, MAX_FRAMES, ParamInfo, kind};

use crate::window::{CLAP, Editor};

fn text(s: Option<&CStr>) -> String {
    s.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn open(path: &Path) -> Result<PluginEntry, String> {
    // SAFETY: loading a plugin runs its code; that's why this runs in its own process.
    unsafe { PluginEntry::load(crate::vst3::binary(path)) }.map_err(|e| format!("{}: {e}", path.display()))
}

pub fn scan(path: &Path) -> Result<Vec<Found>, String> {
    let entry = open(path)?;
    let factory = entry.get_plugin_factory().ok_or("no plugin factory")?;
    Ok(factory
        .plugin_descriptors()
        .map(|d| {
            let features: Vec<String> = d.features().map(|f| f.to_string_lossy().into_owned()).collect();
            Found {
                format: Format::Clap,
                path: path.to_string_lossy().into_owned(),
                id: text(d.id()),
                name: text(d.name()),
                vendor: text(d.vendor()),
                version: text(d.version()),
                instrument: features.iter().any(|f| f == "instrument"),
                features,
            }
        })
        .filter(|f| !f.id.is_empty())
        .collect())
}

// ---------------------------------------------------------------------------
// The host side of the CLAP API
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Exts {
    params: Option<PluginParams>,
    state: Option<PluginState>,
    gui: Option<PluginGui>,
    timer: Option<PluginTimer>,
    #[cfg(unix)]
    posix_fd: Option<PluginPosixFd>,
    audio_ports: Option<PluginAudioPorts>,
    note_ports: Option<PluginNotePorts>,
}

struct Shared {
    main: ThreadId,
    callback: AtomicBool,
    resize: Mutex<Option<GuiSize>>,
    closed: AtomicBool,
    ext: OnceLock<Exts>,
}

impl<'a> SharedHandler<'a> for Shared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {
        self.callback.store(true, Relaxed);
    }
}

impl HostLogImpl for Shared {
    fn log(&self, severity: LogSeverity, message: &str) {
        eprintln!("ravune-host: [{severity}] {message}");
    }
}

impl HostThreadCheckImpl for Shared {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main
    }

    fn is_audio_thread(&self) -> bool {
        !self.is_main_thread()
    }
}

impl HostParamsImplShared for Shared {
    fn request_flush(&self) {} // we process every block anyway
}

impl HostGuiImpl for Shared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, size: GuiSize) -> Result<(), HostError> {
        *self.resize.lock().map_err(|_| HostError::Message("poisoned"))? = Some(size);
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.closed.store(true, Relaxed);
    }
}

struct MainThread<'a> {
    shared: &'a Shared,
    timers: RefCell<Vec<(TimerId, Duration, Instant)>>,
    next_timer: Cell<u32>,
    /// Descriptors the plugin watches (its editor's X connection, on Linux), CLAP's flags.
    #[cfg(unix)]
    fds: RefCell<Vec<(i32, u32)>>,
}

impl<'a> MainThreadHandler<'a> for MainThread<'a> {
    fn initialized(&self, instance: InitializedPluginHandle<'a>) {
        let _ = self.shared.ext.set(Exts {
            params: instance.get_extension(),
            state: instance.get_extension(),
            gui: instance.get_extension(),
            timer: instance.get_extension(),
            #[cfg(unix)]
            posix_fd: instance.get_extension(),
            audio_ports: instance.get_extension(),
            note_ports: instance.get_extension(),
        });
    }
}

impl HostTimerImpl for MainThread<'_> {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        let id = TimerId(self.next_timer.get());
        self.next_timer.set(id.0 + 1);
        let period = Duration::from_millis(period_ms.max(10) as u64);
        self.timers.borrow_mut().push((id, period, Instant::now() + period));
        Ok(id)
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        self.timers.borrow_mut().retain(|t| t.0 != timer_id);
        Ok(())
    }
}

#[cfg(unix)]
impl HostPosixFdImpl for MainThread<'_> {
    fn register_fd(&self, fd: i32, flags: FdFlags) -> Result<(), HostError> {
        let mut fds = self.fds.borrow_mut();
        fds.retain(|f| f.0 != fd);
        fds.push((fd, flags.bits()));
        Ok(())
    }

    fn modify_fd(&self, fd: i32, flags: FdFlags) -> Result<(), HostError> {
        self.register_fd(fd, flags)
    }

    fn unregister_fd(&self, fd: i32) -> Result<(), HostError> {
        self.fds.borrow_mut().retain(|f| f.0 != fd);
        Ok(())
    }
}

impl HostStateImpl for MainThread<'_> {
    fn mark_dirty(&self) {}
}

impl HostParamsImplMainThread for MainThread<'_> {
    fn rescan(&self, _flags: ParamRescanFlags) {}
    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

struct Host;

impl HostHandlers for Host {
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostLog>().register::<HostThreadCheck>().register::<HostParams>().register::<HostState>().register::<HostGui>().register::<HostTimer>();
        #[cfg(unix)]
        builder.register::<HostPosixFd>();
    }
}

type Queue = Arc<Mutex<Vec<(u32, f64)>>>;

/// How notes go in: CLAP note events, or MIDI 1 / MIDI 2 for plugins that take only those.
#[derive(Clone, Copy, PartialEq)]
enum Notes {
    Clap,
    Midi,
    Midi2,
}

// ---------------------------------------------------------------------------
// A loaded plugin (main thread)
// ---------------------------------------------------------------------------

pub struct Clap {
    pub name: String,
    pub instrument: bool,
    instance: PluginInstance<Host>,
    rate: f64,
    max_frames: u32,
    /// Channels of every input and output port (the main one first, as CLAP has it).
    ports: (Vec<u32>, Vec<u32>),
    notes: Notes,
    /// Parameter changes for the audio thread, and the plugin's own (from its editor).
    to_audio: Queue,
    from_audio: Queue,
    editor: Option<Editor>,
    gui_open: bool,
    _entry: PluginEntry,
}

impl Clap {
    pub fn load(path: &Path, id: &str, rate: f64, max_frames: u32) -> Result<Self, String> {
        let entry = open(path)?;
        let factory = entry.get_plugin_factory().ok_or("no plugin factory")?;
        let descriptor = factory.plugin_descriptors().find(|d| d.id().is_some_and(|i| i.to_bytes() == id.as_bytes())).ok_or_else(|| format!("{id} isn't in {}", path.display()))?;
        let name = text(descriptor.name());
        let features: Vec<String> = descriptor.features().map(|f| f.to_string_lossy().into_owned()).collect();
        let info = HostInfo::new("Ravune", "Ravune", "https://github.com/g1lg1l/ravune", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
        let cid = CString::new(id).map_err(|e| e.to_string())?;
        let main = std::thread::current().id();
        let mut instance = PluginInstance::<Host>::new(
            |_| Shared { main, callback: AtomicBool::new(false), resize: Mutex::new(None), closed: AtomicBool::new(false), ext: OnceLock::new() },
            |shared| MainThread {
                shared,
                timers: RefCell::default(),
                next_timer: Cell::new(1),
                #[cfg(unix)]
                fds: RefCell::default(),
            },
            &entry,
            &cid,
            &info,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        let (ports, notes) = {
            let exts = instance.access_shared_handler(|s| s.ext.get().map(|e| (e.audio_ports, e.note_ports)));
            let (audio, notes) = exts.unwrap_or((None, None));
            let handle = instance.plugin_handle();
            // the plugin expects a buffer for every port, not just the main ones
            let list = |is_input: bool| match &audio {
                Some(ports) => {
                    let mut buffer = AudioPortInfoBuffer::new();
                    (0..ports.count(&handle, is_input)).map(|i| ports.get(&handle, i, is_input, &mut buffer).map_or(0, |p| p.channel_count)).collect()
                }
                None if is_input => Vec::new(),
                None => vec![2],
            };
            // the plugin's preferred dialect, else the first we speak of those it takes
            let notes = notes.filter(|n| n.count(&handle, true) > 0).and_then(|n| n.get(&handle, 0, true, &mut NotePortInfoBuffer::new()).map(|p| (p.preferred_dialect, p.supported_dialects))).map(|(preferred, d)| match preferred {
                Some(NoteDialect::Clap) => Notes::Clap,
                Some(NoteDialect::Midi | NoteDialect::MidiMpe) => Notes::Midi,
                Some(NoteDialect::Midi2) => Notes::Midi2,
                None if d.contains(NoteDialects::CLAP) || d.is_empty() => Notes::Clap,
                None if d.intersects(NoteDialects::MIDI | NoteDialects::MIDI_MPE) => Notes::Midi,
                None => Notes::Midi2,
            });
            ((list(true), list(false)), notes)
        };
        let instrument = features.iter().any(|f| f == "instrument") || (ports.0.first().is_none_or(|c| *c == 0) && notes.is_some());
        let notes = notes.unwrap_or(Notes::Clap);
        Ok(Self { name, instrument, instance, rate, max_frames, ports, notes, to_audio: Queue::default(), from_audio: Queue::default(), editor: None, gui_open: false, _entry: entry })
    }

    fn ext<R>(&self, f: impl FnOnce(&Exts) -> Option<R>) -> Option<R> {
        self.instance.access_shared_handler(|s| s.ext.get().and_then(f))
    }

    /// Activates the plugin; its audio half goes to the audio thread.
    pub fn audio(&mut self) -> Result<Audio, String> {
        let config = PluginAudioConfiguration { sample_rate: self.rate, min_frames_count: 1, max_frames_count: self.max_frames.min(MAX_FRAMES as u32) };
        let stopped = self.instance.activate(|_, _| (), config).map_err(|e| e.to_string())?;
        let buffers = |ports: &[u32]| ports.iter().map(|n| (0..*n).map(|_| vec![0.0; MAX_FRAMES]).collect()).collect::<Vec<Vec<Vec<f32>>>>();
        let sum = |ports: &[u32]| ports.iter().sum::<u32>() as usize;
        Ok(Audio {
            processor: Some(Processor::Stopped(stopped)),
            events_in: EventBuffer::with_capacity(1024),
            events_out: EventBuffer::with_capacity(256),
            in_ports: AudioPorts::with_capacity(sum(&self.ports.0), self.ports.0.len()),
            out_ports: AudioPorts::with_capacity(sum(&self.ports.1), self.ports.1.len()),
            inputs: buffers(&self.ports.0),
            outputs: buffers(&self.ports.1),
            notes: self.notes,
            to_audio: self.to_audio.clone(),
            from_audio: self.from_audio.clone(),
            held: [0; 16],
            steady: 0,
        })
    }

    /// Takes the audio half back and deactivates.
    pub fn finish(&mut self, audio: Audio) {
        let stopped = match audio.processor {
            Some(Processor::Started(p)) => p.stop_processing(),
            Some(Processor::Stopped(p)) => p,
            None => return,
        };
        self.instance.deactivate(stopped);
    }

    pub fn params(&mut self) -> Vec<ParamInfo> {
        let Some(params) = self.ext(|e| e.params) else { return Vec::new() };
        let handle = self.instance.plugin_handle();
        let mut buffer = ParamInfoBuffer::new();
        (0..params.count(&handle))
            .filter_map(|i| {
                let info = params.get_info(&handle, i, &mut buffer)?;
                if info.flags.contains(ParamInfoFlags::IS_HIDDEN) {
                    return None;
                }
                let id = info.id;
                let value = params.get_value(&handle, id).unwrap_or(info.default_value);
                Some(ParamInfo {
                    id: id.get(),
                    name: String::from_utf8_lossy(info.name).into_owned(),
                    module: String::from_utf8_lossy(info.module).into_owned(),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    value,
                    stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
                })
            })
            .collect()
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        if let Ok(mut q) = self.to_audio.lock() {
            q.push((id, value));
        }
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let state = self.ext(|e| e.state).ok_or("this plugin keeps no state")?;
        let mut out = Vec::new();
        state.save(&self.instance.plugin_handle(), &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        let state = self.ext(|e| e.state).ok_or("this plugin keeps no state")?;
        state.load(&self.instance.plugin_handle(), &mut Cursor::new(data)).map_err(|e| e.to_string())
    }

    pub fn open_editor(&mut self, title: &str) -> Result<(), String> {
        if self.gui_open {
            if let Some(editor) = &self.editor {
                editor.show();
            }
            return Ok(());
        }
        let gui = self.ext(|e| e.gui).ok_or("this plugin has no editor")?;
        let handle = self.instance.plugin_handle();
        let embedded = GuiConfiguration { api_type: CLAP, is_floating: false };
        if gui.is_api_supported(&handle, embedded) {
            gui.create(&handle, embedded).map_err(|e| e.to_string())?;
            let size = gui.get_size(&handle).unwrap_or(GuiSize { width: 640, height: 400 });
            let editor = Editor::new(title, size.width, size.height)?;
            // SAFETY: the window outlives the editor: we destroy the GUI before dropping it.
            unsafe { gui.set_parent(&handle, Window::from_generic_ptr(CLAP, editor.handle())) }.map_err(|e| e.to_string())?;
            gui.show(&handle).map_err(|e| e.to_string())?;
            editor.show();
            self.editor = Some(editor);
        } else {
            let floating = GuiConfiguration { api_type: GuiApiType::default_for_current_platform().ok_or("no editor here")?, is_floating: true };
            gui.create(&handle, floating).map_err(|_| "the plugin's editor can't open here".to_string())?;
            if let Ok(title) = CString::new(title) {
                gui.suggest_title(&handle, &title);
            }
            gui.show(&handle).map_err(|e| e.to_string())?;
        }
        self.gui_open = true;
        self.instance.access_shared_handler(|s| s.closed.store(false, Relaxed));
        Ok(())
    }

    pub fn close_editor(&mut self) {
        if !self.gui_open {
            return;
        }
        if let Some(gui) = self.ext(|e| e.gui) {
            let handle = self.instance.plugin_handle();
            let _ = gui.hide(&handle);
            gui.destroy(&handle);
        }
        self.editor = None;
        self.gui_open = false;
    }

    pub fn idle(&mut self) -> (Vec<(u32, f64)>, bool) {
        if self.instance.access_shared_handler(|s| s.callback.swap(false, Relaxed)) {
            self.instance.call_on_main_thread_callback();
        }
        #[cfg(unix)]
        if let Some(posix) = self.ext(|e| e.posix_fd) {
            let fds = self.instance.access_handler(|m| m.fds.borrow().clone());
            let handle = self.instance.plugin_handle();
            for (fd, flags) in crate::window::ready(&fds) {
                posix.on_fd(&handle, fd, FdFlags::from_bits_truncate(flags));
            }
        }
        if let Some(timer) = self.ext(|e| e.timer) {
            let now = Instant::now();
            let due: Vec<TimerId> = self.instance.access_handler(|m| {
                let mut timers = m.timers.borrow_mut();
                timers.iter_mut().filter(|t| t.2 <= now).map(|t| {
                    t.2 = now + t.1;
                    t.0
                }).collect()
            });
            let handle = self.instance.plugin_handle();
            for id in due {
                timer.on_timer(&handle, id);
            }
        }
        if let Some(size) = self.instance.access_shared_handler(|s| s.resize.lock().ok().and_then(|mut r| r.take()))
            && let Some(editor) = &self.editor
        {
            editor.resize(size.width, size.height);
            if let Some(gui) = self.ext(|e| e.gui) {
                let _ = gui.set_size(&self.instance.plugin_handle(), size);
            }
        }
        let closed = self.gui_open && (self.editor.as_ref().is_some_and(Editor::closed) || self.instance.access_shared_handler(|s| s.closed.swap(false, Relaxed)));
        let moved = self.from_audio.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        (moved, closed)
    }
}

// ---------------------------------------------------------------------------
// The audio half
// ---------------------------------------------------------------------------

/// A note on (with velocity) or off, in the dialect the plugin takes.
fn note(events: &mut EventBuffer, notes: Notes, at: u32, ch: u16, key: u16, velocity: Option<f64>) {
    let status = if velocity.is_some() { 0x90 } else { 0x80 } | ch as u8;
    match (notes, velocity) {
        (Notes::Clap, Some(v)) => events.push(&NoteOnEvent::new(at, Pckn::new(0u16, ch, key, Match::All), v)),
        (Notes::Clap, None) => events.push(&NoteOffEvent::new(at, Pckn::new(0u16, ch, key, Match::All), 0.0)),
        // velocity 0 would be a note off in MIDI 1
        (Notes::Midi, _) => events.push(&MidiEvent::new(at, 0, [status, key as u8, velocity.map_or(0, |v| (v * 127.0).round().max(1.0) as u8)])),
        // a MIDI 2 channel voice message: type 4, group 0, then 16-bit velocity
        (Notes::Midi2, _) => events.push(&Midi2Event::new(at, 0, [0x4000_0000 | (status as u32) << 16 | (key as u32) << 8, velocity.map_or(0, |v| (v * 65535.0) as u32) << 16, 0, 0])),
    }
}

enum Processor {
    Stopped(StoppedPluginAudioProcessor<Host>),
    Started(StartedPluginAudioProcessor<Host>),
}

pub struct Audio {
    processor: Option<Processor>,
    events_in: EventBuffer,
    events_out: EventBuffer,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    /// Port → channel → samples; the main ports carry the bridge, the others stay silent.
    inputs: Vec<Vec<Vec<f32>>>,
    outputs: Vec<Vec<Vec<f32>>>,
    notes: Notes,
    to_audio: Queue,
    from_audio: Queue,
    /// Keys down per MIDI channel, to let go of on "all notes off".
    held: [u128; 16],
    steady: u64,
}

impl Audio {
    /// Ends processing (on the audio thread, where CLAP wants it).
    pub fn stop(&mut self) {
        if let Some(Processor::Started(p)) = self.processor.take() {
            self.processor = Some(Processor::Stopped(p.stop_processing()));
        }
    }

    fn events(&mut self, block: &Block<'_>) {
        let last = block.frames.saturating_sub(1) as u32;
        self.events_in.clear();
        if let Ok(mut q) = self.to_audio.try_lock() {
            for (id, value) in q.drain(..) {
                if let Some(id) = ClapId::from_raw(id) {
                    self.events_in.push(&ParamValueEvent::new(0, id, Pckn::match_all(), value));
                }
            }
        }
        for e in block.events {
            let at = e.frame.min(last);
            let ch = (e.channel & 15) as usize;
            let key = (e.key & 127) as u16;
            let midi = |status: u8, a: u8, b: u8| MidiEvent::new(at, 0, [status | ch as u8, a & 127, b & 127]);
            match e.kind {
                kind::NOTE_ON => {
                    note(&mut self.events_in, self.notes, at, ch as u16, key, Some(e.value.clamp(0.0, 1.0)));
                    self.held[ch] |= 1 << key;
                }
                kind::NOTE_OFF => {
                    note(&mut self.events_in, self.notes, at, ch as u16, key, None);
                    self.held[ch] &= !(1 << key);
                }
                kind::PARAM => {
                    if let Some(id) = ClapId::from_raw(e.key) {
                        self.events_in.push(&ParamValueEvent::new(at, id, Pckn::match_all(), e.value));
                    }
                }
                kind::BEND => {
                    let v = ((e.value.clamp(-1.0, 1.0) + 1.0) * 0.5 * 16383.0).round() as u16;
                    self.events_in.push(&midi(0xE0, (v & 127) as u8, (v >> 7) as u8));
                }
                kind::PRESSURE => self.events_in.push(&midi(0xD0, (e.value.clamp(0.0, 1.0) * 127.0) as u8, 0)),
                kind::CC => self.events_in.push(&midi(0xB0, e.key as u8, (e.value.clamp(0.0, 1.0) * 127.0) as u8)),
                kind::ALL_OFF => {
                    for (c, keys) in self.held.iter_mut().enumerate() {
                        for k in (0..128u16).filter(|k| *keys >> k & 1 == 1) {
                            note(&mut self.events_in, self.notes, at, c as u16, k, None);
                        }
                        *keys = 0;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn process(&mut self, block: Block<'_>) {
        let frames = block.frames;
        self.events(&block);
        let Block { input: block_in, output: [out_l, out_r], tempo, beats, playing, .. } = block;
        out_l.fill(0.0);
        out_r.fill(0.0);
        let mut started = match self.processor.take() {
            Some(Processor::Started(p)) => p,
            Some(Processor::Stopped(p)) => match p.start_processing() {
                Ok(p) => p,
                Err(e) => {
                    self.processor = Some(Processor::Stopped(e.into_stopped_processor()));
                    return;
                }
            },
            None => return,
        };
        if let Some(main) = self.inputs.first_mut() {
            for (buffer, from) in main.iter_mut().zip(block_in) {
                buffer[..frames].copy_from_slice(&from[..frames]);
            }
        }
        let input = self.in_ports.with_input_buffers(self.inputs.iter_mut().map(|port| AudioPortBuffer { latency: 0, channels: AudioPortBufferType::f32_input_only(port.iter_mut().map(|b| InputChannel::variable(&mut b[..frames]))) }));
        let mut output = self.out_ports.with_output_buffers(self.outputs.iter_mut().map(|port| AudioPortBuffer { latency: 0, channels: AudioPortBufferType::f32_output_only(port.iter_mut().map(|b| &mut b[..frames])) }));
        let mut flags = TransportFlags::HAS_TEMPO | TransportFlags::HAS_BEATS_TIMELINE | TransportFlags::HAS_TIME_SIGNATURE;
        if playing {
            flags |= TransportFlags::IS_PLAYING;
        }
        let transport = TransportEvent {
            header: EventHeader::new_core(0, EventFlags::empty()),
            flags,
            song_pos_beats: BeatTime::from_float(beats),
            song_pos_seconds: SecondsTime::from_float(beats * 60.0 / tempo.max(1.0)),
            tempo,
            tempo_inc: 0.0,
            loop_start_beats: BeatTime::from_float(0.0),
            loop_end_beats: BeatTime::from_float(0.0),
            loop_start_seconds: SecondsTime::from_float(0.0),
            loop_end_seconds: SecondsTime::from_float(0.0),
            bar_start: BeatTime::from_float((beats / 4.0).floor() * 4.0),
            bar_number: (beats / 4.0).floor() as i32,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
        };
        let ok = started
            .process(&input, &mut output, &InputEvents::from_buffer(&self.events_in), &mut OutputEvents::from_buffer(&mut self.events_out), Some(self.steady), Some(&transport))
            .is_ok();
        self.steady += frames as u64;
        if ok && let Some(main) = self.outputs.first().filter(|p| !p.is_empty()) {
            out_l.copy_from_slice(&main[0][..frames]);
            out_r.copy_from_slice(&main[main.len().min(2) - 1][..frames]);
        }
        if let Ok(mut q) = self.from_audio.try_lock() {
            for e in self.events_out.iter() {
                if let Some(CoreEventSpace::ParamValue(p)) = e.as_core_event()
                    && let Some(id) = p.param_id()
                {
                    q.push((id.get(), p.value()));
                }
            }
        }
        self.events_out.clear();
        self.processor = Some(Processor::Started(started));
    }
}
