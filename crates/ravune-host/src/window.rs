//! The window a plugin's editor lives in: Win32 on Windows, Cocoa on macOS, X11 elsewhere
//! (Wayland desktops run it through XWayland), and the host loop's share of each.

pub use imp::*;

#[cfg(windows)]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;

    use clack_extensions::gui::GuiApiType;
    use vst3::Steinberg::{FIDString, kPlatformTypeHWND};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// What CLAP and VST3 call this kind of window.
    pub const CLAP: GuiApiType<'static> = GuiApiType::WIN32;
    pub const VST3: FIDString = kPlatformTypeHWND;

    thread_local! {
        static CLOSED: Cell<bool> = const { Cell::new(false) };
    }

    const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_CLOSE {
            CLOSED.with(|c| c.set(true)); // the host tears the editor down properly
            return 0;
        }
        // SAFETY: forwarding the arguments we were given.
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    fn class() -> *const u16 {
        static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
        NAME.get_or_init(|| {
            let name = wide("RavunePluginEditor");
            // SAFETY: a valid class description; registered once.
            unsafe {
                let class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(std::ptr::null()),
                    hIcon: std::ptr::null_mut(),
                    hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: name.as_ptr(),
                };
                RegisterClassW(&class);
            }
            name
        })
        .as_ptr()
    }

    fn outer(width: u32, height: u32) -> (i32, i32) {
        let mut r = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
        // SAFETY: a valid rect.
        unsafe { AdjustWindowRectEx(&mut r, STYLE, 0, 0) };
        (r.right - r.left, r.bottom - r.top)
    }

    pub struct Editor {
        hwnd: HWND,
    }

    impl Editor {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let (w, h) = outer(width.max(64), height.max(32));
            let title = wide(title);
            // SAFETY: valid class and title strings; a top-level window of ours.
            let hwnd = unsafe {
                CreateWindowExW(0, class(), title.as_ptr(), STYLE, CW_USEDEFAULT, CW_USEDEFAULT, w, h, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
            };
            if hwnd.is_null() {
                return Err("couldn't create the editor window".into());
            }
            CLOSED.with(|c| c.set(false));
            Ok(Self { hwnd })
        }

        pub fn handle(&self) -> *mut c_void {
            self.hwnd
        }

        pub fn show(&self) {
            // SAFETY: our window.
            unsafe {
                ShowWindow(self.hwnd, SW_SHOW);
                SetForegroundWindow(self.hwnd);
            }
        }

        pub fn resize(&self, width: u32, height: u32) {
            resize_window(self.hwnd, width, height);
        }

        /// The user clicked the close box since the last call.
        pub fn closed(&self) -> bool {
            CLOSED.with(|c| c.replace(false))
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            // SAFETY: our window, destroyed once.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Gives an editor window a new client size (plugins ask through their own handle).
    pub fn resize_window(window: *mut c_void, width: u32, height: u32) {
        let (w, h) = outer(width, height);
        // SAFETY: a window of ours (the plugin only knows the ones we gave it).
        unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    /// OLE on the main thread: VST3 editors use it for drag and drop.
    pub fn init() {
        // SAFETY: once, on the main thread, before any window exists.
        unsafe { windows_sys::Win32::System::Ole::OleInitialize(std::ptr::null()) };
    }

    /// Runs the window messages waiting on this (the main) thread.
    pub fn pump() {
        // SAFETY: standard message loop on the thread that owns the windows.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;

    use clack_extensions::gui::GuiApiType;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEventMask, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
    use vst3::Steinberg::{FIDString, kPlatformTypeNSView};

    /// What CLAP and VST3 call this kind of window.
    pub const CLAP: GuiApiType<'static> = GuiApiType::COCOA;
    pub const VST3: FIDString = kPlatformTypeNSView;

    fn main_thread() -> Result<MainThreadMarker, String> {
        MainThreadMarker::new().ok_or_else(|| "editors open on the main thread only".into())
    }

    pub struct Editor {
        window: Retained<NSWindow>,
        view: Retained<NSView>,
        shown: Cell<bool>,
    }

    impl Editor {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let mtm = main_thread()?;
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width.max(64) as f64, height.max(32) as f64));
            let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
            // SAFETY: a plain window of ours on the main thread. The close box only hides it:
            // we let it go ourselves, after the plugin let go of its view.
            let window = unsafe {
                let window = NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false);
                window.setReleasedWhenClosed(false);
                window
            };
            window.setTitle(&NSString::from_str(title));
            window.center();
            let view = window.contentView().ok_or("the editor window has no view")?;
            Ok(Self { window, view, shown: Cell::new(false) })
        }

        pub fn handle(&self) -> *mut c_void {
            Retained::as_ptr(&self.view) as *mut c_void
        }

        pub fn show(&self) {
            self.window.makeKeyAndOrderFront(None);
            #[allow(deprecated)] // activate() needs macOS 14
            NSApplication::sharedApplication(self.window.mtm()).activateIgnoringOtherApps(true);
            self.shown.set(true);
        }

        pub fn resize(&self, width: u32, height: u32) {
            self.window.setContentSize(NSSize::new(width as f64, height as f64));
        }

        /// The user clicked the close box since the last call (it hid the window).
        pub fn closed(&self) -> bool {
            let gone = self.shown.get() && !self.window.isVisible() && !self.window.isMiniaturized();
            if gone {
                self.shown.set(false);
            }
            gone
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            self.window.orderOut(None);
        }
    }

    /// Gives an editor window a new content size (plugins ask through their own view).
    pub fn resize_window(view: *mut c_void, width: u32, height: u32) {
        // SAFETY: a view of ours (the plugin only knows the ones we gave it).
        let view = unsafe { &*(view as *const NSView) };
        if let Some(window) = view.window() {
            window.setContentSize(NSSize::new(width as f64, height as f64));
        }
    }

    /// An app that shows windows without a Dock icon of its own; the host loop drives it.
    pub fn init() {
        if let Ok(mtm) = main_thread() {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
        }
    }

    /// Runs the events waiting on this (the main) thread, and the run loop's timers with them.
    pub fn pump() {
        let Ok(mtm) = main_thread() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let now = NSDate::distantPast();
        // SAFETY: a constant string from Foundation.
        let mode = unsafe { NSDefaultRunLoopMode };
        while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&now), mode, true) {
            app.sendEvent(&event);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use clack_extensions::gui::GuiApiType;
    use vst3::Steinberg::{FIDString, kPlatformTypeX11EmbedWindowID};
    use x11rb::connection::Connection;
    use x11rb::properties::WmSizeHints;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    /// What CLAP and VST3 call this kind of window.
    pub const CLAP: GuiApiType<'static> = GuiApiType::X11;
    pub const VST3: FIDString = kPlatformTypeX11EmbedWindowID;

    thread_local! {
        static CLOSED: Cell<bool> = const { Cell::new(false) };
    }

    /// Our connection to the X server (plugins open their own and draw into our windows).
    struct X {
        conn: RustConnection,
        root: Window,
        black: u32,
        protocols: Atom,
        delete: Atom,
        name: Atom,
        utf8: Atom,
    }

    static DISPLAY: OnceLock<Result<X, String>> = OnceLock::new();

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    fn connect() -> Result<X, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display for the editor: {e}"))?;
        let atom = |name: &str| -> Result<Atom, String> { Ok(conn.intern_atom(false, name.as_bytes()).map_err(err)?.reply().map_err(err)?.atom) };
        let (protocols, delete, name, utf8) = (atom("WM_PROTOCOLS")?, atom("WM_DELETE_WINDOW")?, atom("_NET_WM_NAME")?, atom("UTF8_STRING")?);
        let root = &conn.setup().roots[screen];
        let (root, black) = (root.root, root.black_pixel);
        Ok(X { conn, root, black, protocols, delete, name, utf8 })
    }

    fn x() -> Result<&'static X, String> {
        DISPLAY.get_or_init(connect).as_ref().map_err(Clone::clone)
    }

    /// The size, fixed (editors size themselves; the window follows).
    fn size(x: &X, window: Window, width: u32, height: u32) -> Result<(), String> {
        let fixed = Some((width as i32, height as i32));
        WmSizeHints { min_size: fixed, max_size: fixed, ..Default::default() }.set_normal_hints(&x.conn, window).map_err(err)?;
        x.conn.configure_window(window, &ConfigureWindowAux::new().width(width).height(height)).map_err(err)?;
        x.conn.flush().map_err(err)
    }

    pub struct Editor {
        window: Window,
    }

    impl Editor {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            let x = x()?;
            let (w, h) = (width.max(64), height.max(32));
            let window = x.conn.generate_id().map_err(err)?;
            let aux = CreateWindowAux::new().background_pixel(x.black);
            x.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, window, x.root, 0, 0, w as u16, h as u16, 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &aux).map_err(err)?;
            x.conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes()).map_err(err)?;
            x.conn.change_property8(PropMode::REPLACE, window, x.name, x.utf8, title.as_bytes()).map_err(err)?;
            x.conn.change_property32(PropMode::REPLACE, window, x.protocols, AtomEnum::ATOM, &[x.delete]).map_err(err)?;
            size(x, window, w, h)?;
            // a round trip: the plugin's own connection must find the window there
            x.conn.get_input_focus().map_err(err)?.reply().map_err(err)?;
            CLOSED.with(|c| c.set(false));
            Ok(Self { window })
        }

        pub fn handle(&self) -> *mut c_void {
            self.window as usize as *mut c_void
        }

        pub fn show(&self) {
            if let Ok(x) = x() {
                let _ = x.conn.map_window(self.window);
                let _ = x.conn.configure_window(self.window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                let _ = x.conn.flush();
            }
        }

        pub fn resize(&self, width: u32, height: u32) {
            resize_window(self.handle(), width, height);
        }

        /// The user clicked the close box since the last call.
        pub fn closed(&self) -> bool {
            CLOSED.with(|c| c.replace(false))
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            if let Ok(x) = x() {
                let _ = x.conn.destroy_window(self.window);
                let _ = x.conn.flush();
            }
        }
    }

    /// Gives an editor window a new size (plugins ask through their own handle).
    pub fn resize_window(window: *mut c_void, width: u32, height: u32) {
        if let Ok(x) = x() {
            let _ = size(x, window as usize as Window, width, height);
        }
    }

    pub fn init() {}

    /// Reads what the X server sent our windows: the close box, as WM_DELETE_WINDOW.
    pub fn pump() {
        let Some(Ok(x)) = DISPLAY.get() else { return };
        while let Ok(Some(event)) = x.conn.poll_for_event() {
            if let Event::ClientMessage(e) = event
                && e.type_ == x.protocols
                && e.data.as_data32()[0] == x.delete
            {
                CLOSED.with(|c| c.set(true));
            }
        }
    }
}

/// Which of these descriptors (watched by plugins: Linux editors talk to X through them) are
/// ready now, both ways in CLAP's flags: 1 read, 2 write, 4 error. Doesn't wait.
#[cfg(unix)]
pub fn ready(fds: &[(i32, u32)]) -> Vec<(i32, u32)> {
    let events = |flags: u32| (if flags & 1 != 0 { libc::POLLIN } else { 0 }) | (if flags & 2 != 0 { libc::POLLOUT } else { 0 });
    let mut polled: Vec<libc::pollfd> = fds.iter().map(|&(fd, flags)| libc::pollfd { fd, events: events(flags), revents: 0 }).collect();
    // SAFETY: a valid array of pollfd, and no waiting.
    if unsafe { libc::poll(polled.as_mut_ptr(), polled.len() as libc::nfds_t, 0) } <= 0 {
        return Vec::new();
    }
    let has = |r: i16, bits: i16, flag: u32| if r & bits != 0 { flag } else { 0 };
    polled.iter().filter(|p| p.revents != 0).map(|p| (p.fd, has(p.revents, libc::POLLIN | libc::POLLHUP, 1) | has(p.revents, libc::POLLOUT, 2) | has(p.revents, libc::POLLERR | libc::POLLNVAL, 4))).collect()
}

#[cfg(not(unix))]
pub fn ready(_: &[(i32, u32)]) -> Vec<(i32, u32)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real window wherever there's a display; macOS makes windows on the main thread only,
    /// which tests don't run on.
    #[test]
    fn an_editor_window_opens_and_resizes() {
        if cfg!(target_os = "macos") || (cfg!(unix) && std::env::var_os("DISPLAY").is_none()) {
            return;
        }
        let editor = Editor::new("test", 320, 200).expect("a window");
        assert!(!editor.handle().is_null());
        editor.resize(400, 300);
        pump();
        assert!(!editor.closed());
    }

    #[cfg(unix)]
    #[test]
    fn ready_says_what_a_pipe_can_do() {
        let mut ends = [0; 2];
        // SAFETY: two descriptors of ours, written once, closed at the end.
        unsafe {
            assert_eq!(libc::pipe(ends.as_mut_ptr()), 0);
            assert!(ready(&[(ends[0], 1)]).is_empty(), "nothing to read yet");
            assert_eq!(libc::write(ends[1], b"x".as_ptr().cast(), 1), 1);
            assert_eq!(ready(&[(ends[0], 1), (ends[1], 2)]), vec![(ends[0], 1), (ends[1], 2)]);
            libc::close(ends[0]);
            libc::close(ends[1]);
        }
    }
}
