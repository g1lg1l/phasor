//! VST3 plugins through the vst3 crate's COM bindings: scanning, loading (component and
//! controller, connected), processing (notes; bends, pressure and CCs through the plugin's
//! MIDI mapping; parameters; transport), parameters, state and the editor (IPlugView).
//!
//! The host side of the API is a handful of small COM objects below: the host application
//! (it can make messages and attribute lists), a component handler (edits from the editor),
//! a plug frame (resize requests; on Linux the run loop editors hook into), a memory stream,
//! an event list and parameter queues.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::mem::zeroed;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ravune_bridge::{Block, Format, Found, MAX_FRAMES, ParamInfo, kind};
use vst3::Steinberg::Linux::*;
use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface};

use crate::window::{Editor, VST3, ready, resize_window};

type Queue = Arc<Mutex<Vec<(u32, f64)>>>;

fn text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wide(chars: &[u16]) -> String {
    let end = chars.iter().position(|c| *c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}

fn hex(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

fn unhex(s: &str) -> Option<TUID> {
    let mut out = [0 as c_char; 16];
    if s.len() != 32 {
        return None;
    }
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()? as c_char;
    }
    Some(out)
}

fn iid<I: Interface>() -> FIDString {
    I::IID.as_ptr() as FIDString
}

// ---------------------------------------------------------------------------
// The module (the plugin's library) and its factory
// ---------------------------------------------------------------------------

/// The library inside a plugin bundle (a `.vst3`, or a `.clap` shipped as a folder the same
/// way), or the file itself for single-file plugins.
pub(crate) fn binary(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
    let contents = path.join("Contents");
    if cfg!(windows) {
        let arch = if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" };
        contents.join(arch).join(path.file_name().unwrap_or_default())
    } else if cfg!(target_os = "macos") {
        contents.join("MacOS").join(stem)
    } else {
        let mut file = stem;
        file.push(".so");
        contents.join(format!("{}-linux", std::env::consts::ARCH)).join(file)
    }
}

struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    library: libloading::Library,
}

struct ClassInfo {
    cid: TUID,
    name: String,
    vendor: String,
    version: String,
    subcategories: String,
}

impl Module {
    fn open(path: &Path) -> Result<Self, String> {
        let file = binary(path);
        // SAFETY: loading a plugin runs its code; that's why this runs in its own process.
        let library = unsafe { libloading::Library::new(&file) }.map_err(|e| format!("{}: {e}", file.display()))?;
        // SAFETY: the VST3 module entry points, called as the SDK documents.
        unsafe {
            if let Ok(init) = library.get::<unsafe extern "system" fn() -> bool>(b"InitDll") {
                init();
            }
            let get = library.get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory").map_err(|_| "not a VST3 plugin (no GetPluginFactory)".to_string())?;
            let factory = ComPtr::from_raw(get()).ok_or("the plugin has no factory")?;
            Ok(Self { factory: Some(factory), library })
        }
    }

    fn factory(&self) -> &ComPtr<IPluginFactory> {
        self.factory.as_ref().expect("factory lives as long as the module")
    }

    /// The audio classes in the module.
    fn classes(&self) -> Vec<ClassInfo> {
        let factory = self.factory();
        // SAFETY: plain COM calls with zeroed out-structs.
        unsafe {
            let mut info: PFactoryInfo = zeroed();
            factory.getFactoryInfo(&mut info);
            let vendor = text(&info.vendor);
            let v2 = factory.cast::<IPluginFactory2>();
            (0..factory.countClasses())
                .filter_map(|i| match &v2 {
                    Some(f2) => {
                        let mut c: PClassInfo2 = zeroed();
                        (f2.getClassInfo2(i, &mut c) == kResultOk && text(&c.category) == "Audio Module Class").then(|| ClassInfo {
                            cid: c.cid,
                            name: text(&c.name),
                            vendor: Some(text(&c.vendor)).filter(|v| !v.is_empty()).unwrap_or_else(|| vendor.clone()),
                            version: text(&c.version),
                            subcategories: text(&c.subCategories),
                        })
                    }
                    None => {
                        let mut c: PClassInfo = zeroed();
                        (factory.getClassInfo(i, &mut c) == kResultOk && text(&c.category) == "Audio Module Class").then(|| ClassInfo { cid: c.cid, name: text(&c.name), vendor: vendor.clone(), version: String::new(), subcategories: String::new() })
                    }
                })
                .collect()
        }
    }

    fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
        let mut obj = ptr::null_mut();
        // SAFETY: the factory writes an interface pointer of the requested IID or fails.
        unsafe { (self.factory().createInstance(cid.as_ptr(), iid::<I>(), &mut obj) == kResultOk).then(|| ComPtr::from_raw(obj as *mut I)).flatten() }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        self.factory = None; // release before the library goes
        // SAFETY: the matching exit call; nothing of the plugin is alive anymore.
        unsafe {
            if let Ok(exit) = self.library.get::<unsafe extern "system" fn() -> bool>(b"ExitDll") {
                exit();
            }
        }
    }
}

pub fn scan(path: &Path) -> Result<Vec<Found>, String> {
    let module = Module::open(path)?;
    Ok(module
        .classes()
        .into_iter()
        .map(|c| Found {
            format: Format::Vst3,
            path: path.to_string_lossy().into_owned(),
            id: hex(&c.cid),
            name: c.name,
            vendor: c.vendor,
            version: c.version,
            instrument: c.subcategories.contains("Instrument"),
            features: c.subcategories.split('|').filter(|s| !s.is_empty()).map(String::from).collect(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Host-side COM objects
// ---------------------------------------------------------------------------

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        // SAFETY: the caller passes a String128 to fill.
        let name = unsafe { &mut *name };
        for (dst, src) in name.iter_mut().zip("Ravune".encode_utf16().chain(Some(0))) {
            *dst = src;
        }
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        // SAFETY: cid points at a TUID; obj is the out pointer.
        unsafe {
            let cid: [u8; 16] = std::mem::transmute(*cid);
            let made = if cid == IMessage::IID {
                ComWrapper::new(Message::default()).to_com_ptr::<IMessage>().map(|p| p.into_raw() as *mut c_void)
            } else if cid == IAttributeList::IID {
                ComWrapper::new(Attributes::default()).to_com_ptr::<IAttributeList>().map(|p| p.into_raw() as *mut c_void)
            } else {
                None
            };
            match made {
                Some(p) => {
                    *obj = p;
                    kResultOk
                }
                None => {
                    *obj = ptr::null_mut();
                    kNoInterface
                }
            }
        }
    }
}

#[derive(Default)]
struct Message {
    id: RefCell<Option<CString>>,
    attributes: RefCell<Option<ComPtr<IAttributeList>>>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.borrow().as_ref().map_or(ptr::null(), |s| s.as_ptr())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        // SAFETY: a null-terminated id or null.
        *self.id.borrow_mut() = (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_owned());
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        let mut attributes = self.attributes.borrow_mut();
        let list = attributes.get_or_insert_with(|| ComWrapper::new(Attributes::default()).to_com_ptr::<IAttributeList>().expect("attribute list"));
        list.as_ptr()
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[derive(Default)]
struct Attributes(RefCell<HashMap<String, Attr>>);

impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}

fn key(id: AttrID) -> Option<String> {
    // SAFETY: attribute ids are null-terminated strings.
    (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned())
}

impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        key(id).map_or(kInvalidArgument, |k| {
            self.0.borrow_mut().insert(k, Attr::Int(value));
            kResultOk
        })
    }

    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        match key(id).and_then(|k| match self.0.borrow().get(&k) {
            Some(Attr::Int(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) => {
                // SAFETY: out pointer from the caller.
                unsafe { *value = v };
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        key(id).map_or(kInvalidArgument, |k| {
            self.0.borrow_mut().insert(k, Attr::Float(value));
            kResultOk
        })
    }

    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        match key(id).and_then(|k| match self.0.borrow().get(&k) {
            Some(Attr::Float(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) => {
                // SAFETY: out pointer from the caller.
                unsafe { *value = v };
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        let Some(k) = key(id) else { return kInvalidArgument };
        let mut chars = Vec::new();
        // SAFETY: a null-terminated UTF-16 string.
        unsafe {
            let mut p = string;
            while !p.is_null() && *p != 0 {
                chars.push(*p);
                p = p.add(1);
            }
        }
        self.0.borrow_mut().insert(k, Attr::Str(chars));
        kResultOk
    }

    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size: uint32) -> tresult {
        let map = self.0.borrow();
        let Some(Attr::Str(chars)) = key(id).and_then(|k| map.get(&k)) else { return kResultFalse };
        let room = (size as usize / 2).saturating_sub(1);
        // SAFETY: the caller's buffer holds `size` bytes.
        unsafe {
            for (i, c) in chars.iter().take(room).enumerate() {
                *string.add(i) = *c;
            }
            *string.add(chars.len().min(room)) = 0;
        }
        kResultOk
    }

    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size: uint32) -> tresult {
        let Some(k) = key(id) else { return kInvalidArgument };
        // SAFETY: `size` readable bytes at `data`.
        let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) }.to_vec();
        self.0.borrow_mut().insert(k, Attr::Bin(bytes));
        kResultOk
    }

    unsafe fn getBinary(&self, id: AttrID, data: *mut *const c_void, size: *mut uint32) -> tresult {
        let map = self.0.borrow();
        let Some(Attr::Bin(bytes)) = key(id).and_then(|k| map.get(&k)) else { return kResultFalse };
        // SAFETY: out pointers; the bytes live as long as the list (as the API expects).
        unsafe {
            *data = bytes.as_ptr() as *const c_void;
            *size = bytes.len() as uint32;
        }
        kResultOk
    }
}

/// Edits from the plugin's editor go to the processor and back to Ravune.
struct Handler {
    to_audio: Queue,
    moved: Queue,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if let Ok(mut q) = self.to_audio.lock() {
            q.push((id, value));
        }
        if let Ok(mut q) = self.moved.lock() {
            q.push((id, value));
        }
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// The editor asking for a new size: done on the spot (it's the UI thread). Linux editors
/// also hook their X connection and timers into our loop through it (IRunLoop).
struct Frame {
    window: Cell<*mut c_void>,
    handlers: RefCell<Vec<(ComPtr<IEventHandler>, FileDescriptor)>>,
    timers: RefCell<Vec<(ComPtr<ITimerHandler>, Duration, Instant)>>,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame, IRunLoop);
}

impl Frame {
    fn new() -> Self {
        Self { window: Cell::new(ptr::null_mut()), handlers: RefCell::default(), timers: RefCell::default() }
    }

    /// Calls the handlers whose descriptor has something and the timers that are due (copies
    /// first: a handler may register or unregister from inside its call).
    fn run_loop(&self) {
        let handlers = self.handlers.borrow().clone();
        if !handlers.is_empty() {
            let fds: Vec<(i32, u32)> = handlers.iter().map(|h| (h.1, 1)).collect();
            for (fd, _) in ready(&fds) {
                for (handler, _) in handlers.iter().filter(|h| h.1 == fd) {
                    // SAFETY: a handler the plugin registered and we still hold.
                    unsafe { handler.onFDIsSet(fd) };
                }
            }
        }
        let now = Instant::now();
        let due: Vec<ComPtr<ITimerHandler>> = self
            .timers
            .borrow_mut()
            .iter_mut()
            .filter(|t| t.2 <= now)
            .map(|t| {
                t.2 = now + t.1;
                t.0.clone()
            })
            .collect();
        for timer in due {
            // SAFETY: as above.
            unsafe { timer.onTimer() };
        }
    }
}

impl IRunLoopTrait for Frame {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> tresult {
        // SAFETY: the plugin's handler; we hold a reference until it unregisters.
        let Some(handler) = (unsafe { ComRef::from_raw(handler) }) else { return kInvalidArgument };
        self.handlers.borrow_mut().push((handler.to_com_ptr(), fd));
        kResultOk
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> tresult {
        self.handlers.borrow_mut().retain(|h| h.0.as_ptr() != handler);
        kResultOk
    }

    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: TimerInterval) -> tresult {
        // SAFETY: as above.
        let Some(handler) = (unsafe { ComRef::from_raw(handler) }) else { return kInvalidArgument };
        let period = Duration::from_millis(milliseconds.max(10));
        self.timers.borrow_mut().push((handler.to_com_ptr(), period, Instant::now() + period));
        kResultOk
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        self.timers.borrow_mut().retain(|t| t.0.as_ptr() != handler);
        kResultOk
    }
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, size: *mut ViewRect) -> tresult {
        // SAFETY: valid view and rect from the plugin.
        unsafe {
            let r = &*size;
            let window = self.window.get();
            if !window.is_null() {
                resize_window(window, (r.right - r.left) as u32, (r.bottom - r.top) as u32);
            }
            if let Some(view) = vst3::ComRef::from_raw(view) {
                view.onSize(size);
            }
        }
        kResultOk
    }
}

/// An IBStream over bytes (state in and out).
struct Stream(RefCell<Cursor<Vec<u8>>>);

impl Class for Stream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for Stream {
    unsafe fn read(&self, buffer: *mut c_void, n: int32, read: *mut int32) -> tresult {
        // SAFETY: `n` writable bytes at `buffer`.
        let out = unsafe { std::slice::from_raw_parts_mut(buffer as *mut u8, n.max(0) as usize) };
        let got = self.0.borrow_mut().read(out).unwrap_or(0);
        if !read.is_null() {
            // SAFETY: optional out pointer.
            unsafe { *read = got as int32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, n: int32, written: *mut int32) -> tresult {
        // SAFETY: `n` readable bytes at `buffer`.
        let data = unsafe { std::slice::from_raw_parts(buffer as *const u8, n.max(0) as usize) };
        let put = self.0.borrow_mut().write(data).unwrap_or(0);
        if !written.is_null() {
            // SAFETY: optional out pointer.
            unsafe { *written = put as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        use IBStream_::IStreamSeekMode_::*;
        let to = match mode as IBStream_::IStreamSeekMode {
            m if m == kIBSeekSet => SeekFrom::Start(pos.max(0) as u64),
            m if m == kIBSeekCur => SeekFrom::Current(pos),
            m if m == kIBSeekEnd => SeekFrom::End(pos),
            _ => return kInvalidArgument,
        };
        match self.0.borrow_mut().seek(to) {
            Ok(p) => {
                if !result.is_null() {
                    // SAFETY: optional out pointer.
                    unsafe { *result = p as int64 };
                }
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            // SAFETY: out pointer.
            unsafe { *pos = self.0.borrow().position() as int64 };
        }
        kResultOk
    }
}

fn stream(bytes: Vec<u8>) -> ComWrapper<Stream> {
    ComWrapper::new(Stream(RefCell::new(Cursor::new(bytes))))
}

fn stream_ptr(s: &ComWrapper<Stream>) -> *mut IBStream {
    s.as_com_ref::<IBStream>().expect("IBStream").as_ptr()
}

/// Events for one block. Only the audio thread touches it.
struct Events(UnsafeCell<Vec<Event>>);

impl Class for Events {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> int32 {
        // SAFETY: audio thread only, no other borrow alive.
        unsafe { (*self.0.get()).len() as int32 }
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        // SAFETY: as above; e is the caller's out struct.
        unsafe {
            match (&*self.0.get()).get(index as usize) {
                Some(ev) => {
                    *e = *ev;
                    kResultOk
                }
                None => kInvalidArgument,
            }
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        // SAFETY: as above.
        unsafe { (*self.0.get()).push(*e) };
        kResultOk
    }
}

/// One parameter's changes in a block.
struct ParamQueue {
    id: Cell<ParamID>,
    points: UnsafeCell<Vec<(int32, f64)>>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        // SAFETY: audio thread only.
        unsafe { (*self.points.get()).len() as int32 }
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        // SAFETY: audio thread only; out pointers from the caller.
        unsafe {
            match (&*self.points.get()).get(index as usize) {
                Some(&(o, v)) => {
                    *offset = o;
                    *value = v;
                    kResultOk
                }
                None => kInvalidArgument,
            }
        }
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        // SAFETY: audio thread only.
        unsafe {
            let points = &mut *self.points.get();
            points.push((offset, value));
            if !index.is_null() {
                *index = points.len() as int32 - 1;
            }
        }
        kResultOk
    }
}

/// A block's parameter changes: a fixed pool of queues, reused every block.
struct Changes {
    queues: Vec<ComWrapper<ParamQueue>>,
    used: Cell<usize>,
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl Changes {
    fn new() -> Self {
        Self { queues: (0..64).map(|_| ComWrapper::new(ParamQueue { id: Cell::new(0), points: UnsafeCell::new(Vec::with_capacity(16)) })).collect(), used: Cell::new(0) }
    }

    fn clear(&self) {
        for q in &self.queues[..self.used.get()] {
            // SAFETY: audio thread only.
            unsafe { (*q.points.get()).clear() };
        }
        self.used.set(0);
    }

    fn queue(&self, id: ParamID) -> Option<&ComWrapper<ParamQueue>> {
        let used = self.used.get();
        if let Some(q) = self.queues[..used].iter().find(|q| q.id.get() == id) {
            return Some(q);
        }
        let q = self.queues.get(used)?;
        q.id.set(id);
        self.used.set(used + 1);
        Some(q)
    }

    fn add(&self, id: ParamID, offset: int32, value: f64) {
        if let Some(q) = self.queue(id) {
            // SAFETY: audio thread only.
            unsafe { (*q.points.get()).push((offset, value)) };
        }
    }

    /// The last value of every changed parameter.
    fn last(&self) -> impl Iterator<Item = (u32, f64)> + '_ {
        // SAFETY: audio thread only.
        self.queues[..self.used.get()].iter().filter_map(|q| unsafe { (*q.points.get()).last().map(|p| (q.id.get(), p.1)) })
    }
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        self.queues.get(index as usize).filter(|_| (index as usize) < self.used.get()).map_or(ptr::null_mut(), |q| q.as_com_ref::<IParamValueQueue>().expect("queue").as_ptr())
    }

    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        // SAFETY: id points at a ParamID; index is an optional out pointer.
        unsafe {
            let Some(q) = self.queue(*id) else { return ptr::null_mut() };
            if !index.is_null() {
                *index = self.queues.iter().position(|x| std::ptr::eq(&**x, &**q)).unwrap_or(0) as int32;
            }
            q.as_com_ref::<IParamValueQueue>().expect("queue").as_ptr()
        }
    }
}

// ---------------------------------------------------------------------------
// A loaded plugin (main thread)
// ---------------------------------------------------------------------------

pub struct Vst3 {
    pub name: String,
    pub instrument: bool,
    // fields drop in order: the interfaces a plugin hands out go before the objects behind them
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    component: ComPtr<IComponent>,
    /// The controller is the component itself (no separate controller to terminate).
    single: bool,
    /// Kept alive for the plugin, which holds pointers to them.
    _host: ComWrapper<HostApp>,
    _handler: ComWrapper<Handler>,
    frame: ComWrapper<Frame>,
    view: Option<ComPtr<IPlugView>>,
    editor: Option<Editor>,
    to_audio: Queue,
    moved: Queue,
    /// Channels per audio bus, inputs and outputs.
    buses: (Vec<i32>, Vec<i32>),
    /// MIDI channel and controller (128 aftertouch, 129 pitch bend) → parameter.
    midi: HashMap<(u8, u16), ParamID>,
    rate: f64,
    _module: Module,
}

impl Vst3 {
    pub fn load(path: &Path, id: &str, rate: f64, max_frames: u32) -> Result<Self, String> {
        let module = Module::open(path)?;
        let cid = unhex(id).ok_or_else(|| format!("bad VST3 class id {id}"))?;
        let info = module.classes().into_iter().find(|c| c.cid == cid).ok_or_else(|| format!("{id} isn't in {}", path.display()))?;
        let component = module.create::<IComponent>(&cid).ok_or("the plugin wouldn't create its component")?;
        let host = ComWrapper::new(HostApp);
        let context = host.as_com_ref::<IHostApplication>().expect("host").as_ptr() as *mut FUnknown;
        let to_audio = Queue::default();
        let moved = Queue::default();
        let handler = ComWrapper::new(Handler { to_audio: to_audio.clone(), moved: moved.clone() });
        // SAFETY: the documented VST3 setup sequence on the main thread.
        unsafe {
            if component.initialize(context) != kResultOk {
                return Err(format!("{}: initialize failed", info.name));
            }
            let (controller, single) = match component.cast::<IEditController>() {
                Some(c) => (Some(c), true),
                None => {
                    let mut ccid: TUID = zeroed();
                    let separate = (component.getControllerClassId(&mut ccid) == kResultOk).then(|| module.create::<IEditController>(&ccid)).flatten();
                    if let Some(c) = &separate {
                        c.initialize(context);
                    }
                    (separate, false)
                }
            };
            let connection = match (&controller, single) {
                (Some(c), false) => component.cast::<IConnectionPoint>().zip(c.cast::<IConnectionPoint>()),
                _ => None,
            };
            if let Some((a, b)) = &connection {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
            }
            if let Some(c) = &controller {
                c.setComponentHandler(handler.as_com_ref::<IComponentHandler>().expect("handler").as_ptr());
                let state = stream(Vec::new());
                if component.getState(stream_ptr(&state)) == kResultOk {
                    state.0.borrow_mut().set_position(0);
                    c.setComponentState(stream_ptr(&state));
                }
            }
            let processor = component.cast::<IAudioProcessor>().ok_or("not an audio processor")?;
            use BusDirections_::*;
            use MediaTypes_::*;
            let channels = |dir| {
                (0..component.getBusCount(kAudio as MediaType, dir as BusDirection))
                    .map(|i| {
                        let mut bus: BusInfo = zeroed();
                        component.getBusInfo(kAudio as MediaType, dir as BusDirection, i, &mut bus);
                        bus.channelCount
                    })
                    .collect::<Vec<i32>>()
            };
            let mut buses = (channels(kInput), channels(kOutput));
            // stereo main buses where the plugin agrees
            let arrangement = |c: &Vec<i32>| c.iter().enumerate().map(|(i, n)| if i == 0 && *n > 0 { SpeakerArr::kStereo } else if *n == 1 { SpeakerArr::kMono } else { SpeakerArr::kStereo }).collect::<Vec<SpeakerArrangement>>();
            let (mut ins, mut outs) = (arrangement(&buses.0), arrangement(&buses.1));
            if processor.setBusArrangements(ins.as_mut_ptr(), ins.len() as i32, outs.as_mut_ptr(), outs.len() as i32) == kResultTrue {
                for (n, a) in buses.0.iter_mut().zip(&ins).chain(buses.1.iter_mut().zip(&outs)) {
                    *n = if *a == SpeakerArr::kMono { 1 } else { 2 };
                }
            }
            for (dir, list) in [(kInput, &buses.0), (kOutput, &buses.1)] {
                for i in 0..list.len() {
                    component.activateBus(kAudio as MediaType, dir as BusDirection, i as i32, 1);
                }
            }
            let event_inputs = component.getBusCount(kEvent as MediaType, kInput as BusDirection);
            if event_inputs > 0 {
                component.activateBus(kEvent as MediaType, kInput as BusDirection, 0, 1);
            }
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                maxSamplesPerBlock: max_frames.min(MAX_FRAMES as u32) as int32,
                sampleRate: rate,
            };
            if processor.setupProcessing(&mut setup) != kResultOk {
                return Err(format!("{}: setupProcessing failed", info.name));
            }
            if component.setActive(1) != kResultOk {
                return Err(format!("{}: activation failed", info.name));
            }
            let mut midi = HashMap::new();
            if let Some(mapping) = controller.as_ref().and_then(|c| c.cast::<IMidiMapping>()) {
                for channel in 0..16u8 {
                    for cc in 0..130u16 {
                        let mut param: ParamID = 0;
                        if mapping.getMidiControllerAssignment(0, channel as i16, cc as i16, &mut param) == kResultTrue {
                            midi.insert((channel, cc), param);
                        }
                    }
                }
            }
            let instrument = info.subcategories.contains("Instrument") || (buses.0.is_empty() && event_inputs > 0);
            Ok(Self {
                name: info.name,
                instrument,
                component,
                controller,
                single,
                connection,
                processor,
                _host: host,
                _handler: handler,
                frame: ComWrapper::new(Frame::new()),
                view: None,
                editor: None,
                to_audio,
                moved,
                buses,
                midi,
                rate,
                _module: module,
            })
        }
    }

    pub fn audio(&mut self) -> Result<Audio, String> {
        let bus_buffers = |list: &[i32]| list.iter().map(|n| (0..*n).map(|_| vec![0.0f32; MAX_FRAMES]).collect::<Vec<_>>()).collect::<Vec<_>>();
        Ok(Audio {
            processor: Shared(self.processor.clone()),
            started: false,
            inputs: bus_buffers(&self.buses.0),
            outputs: bus_buffers(&self.buses.1),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_buses: Vec::new(),
            out_buses: Vec::new(),
            events: ComWrapper::new(Events(UnsafeCell::new(Vec::with_capacity(1024)))),
            out_events: ComWrapper::new(Events(UnsafeCell::new(Vec::with_capacity(256)))),
            changes: ComWrapper::new(Changes::new()),
            out_changes: ComWrapper::new(Changes::new()),
            to_audio: self.to_audio.clone(),
            moved: self.moved.clone(),
            midi: self.midi.clone(),
            held: [0; 16],
            rate: self.rate,
            frames: 0,
        })
    }

    pub fn finish(&mut self, audio: Audio) {
        // SAFETY: processing stops before deactivation, as the SDK asks.
        unsafe {
            if audio.started {
                self.processor.setProcessing(0);
            }
            self.component.setActive(0);
        }
    }

    pub fn params(&mut self) -> Vec<ParamInfo> {
        let Some(c) = &self.controller else { return Vec::new() };
        // SAFETY: plain controller queries on the main thread.
        unsafe {
            (0..c.getParameterCount())
                .filter_map(|i| {
                    let mut info: vst3::Steinberg::Vst::ParameterInfo = zeroed();
                    if c.getParameterInfo(i, &mut info) != kResultOk {
                        return None;
                    }
                    use ParameterInfo_::ParameterFlags_::*;
                    if info.flags & (kIsHidden | kIsReadOnly) != 0 {
                        return None;
                    }
                    Some(ParamInfo {
                        id: info.id,
                        name: wide(&info.title),
                        module: String::new(),
                        min: 0.0,
                        max: 1.0,
                        default: info.defaultNormalizedValue,
                        value: c.getParamNormalized(info.id),
                        stepped: info.stepCount > 0,
                    })
                })
                .collect()
        }
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        if let Some(c) = &self.controller {
            // SAFETY: main-thread controller call.
            unsafe { c.setParamNormalized(id, value) };
        }
        if let Ok(mut q) = self.to_audio.lock() {
            q.push((id, value));
        }
    }

    /// "PHV3", component state length (u32 LE), component state, controller state.
    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let component = stream(Vec::new());
        let controller = stream(Vec::new());
        // SAFETY: state calls on the main thread with our streams.
        unsafe {
            if self.component.getState(stream_ptr(&component)) != kResultOk {
                return Err("the plugin couldn't save its state".into());
            }
            if let Some(c) = &self.controller
                && !self.single
            {
                c.getState(stream_ptr(&controller));
            }
        }
        let (a, b) = (component.0.borrow().get_ref().clone(), controller.0.borrow().get_ref().clone());
        let mut out = b"PHV3".to_vec();
        out.extend((a.len() as u32).to_le_bytes());
        out.extend(a);
        out.extend(b);
        Ok(out)
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        let rest = data.strip_prefix(b"PHV3").ok_or("not a Ravune VST3 state")?;
        let len = u32::from_le_bytes(rest.get(..4).ok_or("truncated state")?.try_into().unwrap()) as usize;
        let component = rest.get(4..4 + len).ok_or("truncated state")?.to_vec();
        let controller = rest[4 + len..].to_vec();
        let s = stream(component);
        // SAFETY: state calls on the main thread with our streams.
        unsafe {
            if self.component.setState(stream_ptr(&s)) != kResultOk {
                return Err("the plugin refused the saved state".into());
            }
            if let Some(c) = &self.controller {
                s.0.borrow_mut().set_position(0);
                c.setComponentState(stream_ptr(&s));
                if !self.single && !controller.is_empty() {
                    c.setState(stream_ptr(&stream(controller)));
                }
            }
        }
        Ok(())
    }

    pub fn open_editor(&mut self, title: &str) -> Result<(), String> {
        if let Some(editor) = &self.editor {
            editor.show();
            return Ok(());
        }
        let c = self.controller.as_ref().ok_or("this plugin has no editor")?;
        // SAFETY: the documented IPlugView sequence on the UI thread; the window outlives the
        // attachment (removed() comes first in close_editor).
        unsafe {
            let view = ComPtr::from_raw(c.createView(c"editor".as_ptr())).ok_or("this plugin has no editor")?;
            if view.isPlatformTypeSupported(VST3) != kResultTrue {
                return Err("the plugin's editor can't open here".into());
            }
            let mut rect: ViewRect = zeroed();
            view.getSize(&mut rect);
            let editor = Editor::new(title, (rect.right - rect.left).max(64) as u32, (rect.bottom - rect.top).max(32) as u32)?;
            self.frame.window.set(editor.handle());
            view.setFrame(self.frame.as_com_ref::<IPlugFrame>().expect("frame").as_ptr());
            if view.attached(editor.handle(), VST3) != kResultOk {
                view.setFrame(ptr::null_mut());
                return Err("the plugin's editor didn't open".into());
            }
            editor.show();
            self.view = Some(view);
            self.editor = Some(editor);
        }
        Ok(())
    }

    pub fn close_editor(&mut self) {
        if let Some(view) = self.view.take() {
            // SAFETY: detaching before the window goes.
            unsafe {
                view.removed();
                view.setFrame(ptr::null_mut());
            }
        }
        self.frame.window.set(ptr::null_mut());
        self.editor = None;
    }

    pub fn idle(&mut self) -> (Vec<(u32, f64)>, bool) {
        self.frame.run_loop();
        let closed = self.editor.as_ref().is_some_and(Editor::closed);
        let moved = self.moved.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        (moved, closed)
    }
}

impl Drop for Vst3 {
    fn drop(&mut self) {
        self.close_editor();
        // SAFETY: the documented teardown, in reverse.
        unsafe {
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(ptr::null_mut());
                if !self.single {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

// ---------------------------------------------------------------------------
// The audio half
// ---------------------------------------------------------------------------

/// The processor, used from the audio thread only (VST3 allows that for process calls).
struct Shared(ComPtr<IAudioProcessor>);

// SAFETY: see above; the main thread doesn't touch it while the audio thread runs.
unsafe impl Send for Shared {}

pub struct Audio {
    processor: Shared,
    started: bool,
    inputs: Vec<Vec<Vec<f32>>>,
    outputs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    events: ComWrapper<Events>,
    out_events: ComWrapper<Events>,
    changes: ComWrapper<Changes>,
    out_changes: ComWrapper<Changes>,
    to_audio: Queue,
    moved: Queue,
    midi: HashMap<(u8, u16), ParamID>,
    held: [u128; 16],
    rate: f64,
    frames: i64,
}

// SAFETY: everything in here is used by the audio thread alone once it's moved there.
unsafe impl Send for Audio {}

fn note(kind: u16, frame: u32, channel: u32, key: u32, velocity: f32) -> Event {
    // SAFETY: Event is plain data; zero is a valid starting point.
    let mut e: Event = unsafe { zeroed() };
    e.sampleOffset = frame as int32;
    e.r#type = kind;
    if kind == Event_::EventTypes_::kNoteOnEvent as u16 {
        e.__field0.noteOn = NoteOnEvent { channel: channel as int16, pitch: key as int16, tuning: 0.0, velocity, length: 0, noteId: -1 };
    } else {
        e.__field0.noteOff = NoteOffEvent { channel: channel as int16, pitch: key as int16, velocity, noteId: -1, tuning: 0.0 };
    }
    e
}

impl Audio {
    pub fn process(&mut self, block: Block<'_>) {
        let frames = block.frames;
        let Block { events, input, output: [out_l, out_r], tempo, beats, playing, .. } = block;
        out_l.fill(0.0);
        out_r.fill(0.0);
        let p = &self.processor.0;
        // SAFETY: audio-thread calls on the processor and our own COM objects.
        unsafe {
            if !self.started {
                // the buffers never move again, so their pointers are set up once
                self.in_ptrs = self.inputs.iter_mut().map(|bus| bus.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
                self.out_ptrs = self.outputs.iter_mut().map(|bus| bus.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
                let buses = |ptrs: &mut Vec<Vec<*mut f32>>| {
                    ptrs.iter_mut()
                        .map(|p| {
                            let mut b: AudioBusBuffers = zeroed();
                            b.numChannels = p.len() as int32;
                            b.__field0.channelBuffers32 = p.as_mut_ptr();
                            b
                        })
                        .collect::<Vec<_>>()
                };
                self.in_buses = buses(&mut self.in_ptrs);
                self.out_buses = buses(&mut self.out_ptrs);
                p.setProcessing(1);
                self.started = true;
            }
            let list = &mut *self.events.0.get();
            list.clear();
            self.changes.clear();
            self.out_changes.clear();
            (*self.out_events.0.get()).clear();
            if let Ok(mut q) = self.to_audio.try_lock() {
                for (id, value) in q.drain(..) {
                    self.changes.add(id, 0, value);
                }
            }
            use Event_::EventTypes_::*;
            for e in events {
                let at = e.frame.min(frames.saturating_sub(1) as u32);
                let ch = (e.channel & 15) as usize;
                let mapped = |cc: u16| self.midi.get(&(ch as u8, cc)).copied();
                match e.kind {
                    kind::NOTE_ON => {
                        list.push(note(kNoteOnEvent as u16, at, ch as u32, e.key & 127, e.value as f32));
                        self.held[ch] |= 1 << (e.key & 127);
                    }
                    kind::NOTE_OFF => {
                        list.push(note(kNoteOffEvent as u16, at, ch as u32, e.key & 127, 0.0));
                        self.held[ch] &= !(1 << (e.key & 127));
                    }
                    kind::PARAM => self.changes.add(e.key, at as int32, e.value),
                    kind::BEND => {
                        if let Some(id) = mapped(129) {
                            self.changes.add(id, at as int32, (e.value.clamp(-1.0, 1.0) + 1.0) * 0.5);
                        }
                    }
                    kind::PRESSURE => {
                        if let Some(id) = mapped(128) {
                            self.changes.add(id, at as int32, e.value.clamp(0.0, 1.0));
                        }
                    }
                    kind::CC => {
                        if let Some(id) = mapped(e.key as u16) {
                            self.changes.add(id, at as int32, e.value.clamp(0.0, 1.0));
                        }
                    }
                    kind::ALL_OFF => {
                        for (c, keys) in self.held.iter_mut().enumerate() {
                            for k in (0..128u32).filter(|k| *keys >> k & 1 == 1) {
                                list.push(note(kNoteOffEvent as u16, at, c as u32, k, 0.0));
                            }
                            *keys = 0;
                        }
                    }
                    _ => {}
                }
            }
            // buses: the main input gets our audio, the rest (sidechains, extra outs) scratch
            for (b, bus) in self.inputs.iter_mut().enumerate() {
                for (c, ch) in bus.iter_mut().enumerate() {
                    match input.get(c).filter(|_| b == 0) {
                        Some(src) => ch[..frames].copy_from_slice(&src[..frames]),
                        None => ch[..frames].fill(0.0),
                    }
                }
            }
            let mut context: ProcessContext = zeroed();
            use ProcessContext_::StatesAndFlags_::*;
            // the flags are i32 on Windows and u32 elsewhere: the casts matter on one only
            #[allow(clippy::unnecessary_cast)]
            let state = (kTempoValid | kProjectTimeMusicValid | kBarPositionValid | kTimeSigValid) as u32 | if playing { kPlaying as u32 } else { 0 };
            context.state = state;
            context.sampleRate = self.rate;
            context.projectTimeSamples = (beats * 60.0 / tempo.max(1.0) * self.rate) as i64;
            context.continousTimeSamples = self.frames;
            context.projectTimeMusic = beats;
            context.barPositionMusic = (beats / 4.0).floor() * 4.0;
            context.tempo = tempo;
            context.timeSigNumerator = 4;
            context.timeSigDenominator = 4;
            let mut data = ProcessData {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                numSamples: frames as int32,
                numInputs: self.in_buses.len() as int32,
                numOutputs: self.out_buses.len() as int32,
                inputs: if self.in_buses.is_empty() { ptr::null_mut() } else { self.in_buses.as_mut_ptr() },
                outputs: if self.out_buses.is_empty() { ptr::null_mut() } else { self.out_buses.as_mut_ptr() },
                inputParameterChanges: self.changes.as_com_ref::<IParameterChanges>().expect("changes").as_ptr(),
                outputParameterChanges: self.out_changes.as_com_ref::<IParameterChanges>().expect("changes").as_ptr(),
                inputEvents: self.events.as_com_ref::<IEventList>().expect("events").as_ptr(),
                outputEvents: self.out_events.as_com_ref::<IEventList>().expect("events").as_ptr(),
                processContext: &mut context,
            };
            if p.process(&mut data) == kResultOk
                && let Some(main) = self.outputs.first()
            {
                if let Some(l) = main.first() {
                    out_l.copy_from_slice(&l[..frames]);
                }
                if let Some(r) = main.get(1).or(main.first()) {
                    out_r.copy_from_slice(&r[..frames]);
                }
            }
            if let Ok(mut q) = self.moved.try_lock() {
                q.extend(self.out_changes.last());
            }
        }
        self.frames += frames as i64;
    }
}
