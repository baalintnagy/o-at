//! The editor window: an egui window inside the host's that the analyzer resizes itself,
//! by its resize corner and FULL SCREEN.
//!
//! This follows `nih_plug_egui`'s editor, which keeps its resize request private. Hosts
//! cannot resize NIH-plug editors from their side at the pinned revision (the wrappers
//! answer `can_resize` with false), so every size change starts here.
//!
//! FULL SCREEN takes over the host's plugin window: it removes the window's frame, covers
//! the monitor and sizes the editor to match. RESTORE puts frame, place and size back.
//! This works on Windows; elsewhere the button does nothing.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use baseview::gl::GlConfig;
use baseview::{PhySize, Size, WindowHandle, WindowOpenOptions, WindowScalePolicy};
use egui::{Context, Vec2, ViewportCommand};
use egui_baseview::{EguiWindow, GraphicsConfig};
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::{Editor, GuiContext, ParentWindowHandle};
use raw_window_handle::{
    AppKitWindowHandle, HasRawWindowHandle, RawWindowHandle, Win32WindowHandle, XcbWindowHandle,
};
use serde::{Deserialize, Serialize};

/// The stored part of the window state, in the format `nih_plug_egui` used, so sessions
/// keep their editor size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredWindow {
    /// Logical size in pixels.
    pub size: (u32, u32),
}

/// Full-screen state: the editor size and host window to restore.
#[derive(Debug)]
struct FullScreen {
    size: (u32, u32),
    host: screen::Saved,
}

/// Editor size, resize requests and full screen.
#[derive(Debug)]
pub struct WindowState {
    size: Mutex<(u32, u32)>,
    requested: Mutex<Option<(u32, u32)>>,
    full: Mutex<Option<FullScreen>>,
    /// A full-screen change to make on the GUI thread: `true` enters, `false` leaves.
    pending: Mutex<Option<bool>>,
    min_size: (u32, u32),
    open: AtomicBool,
    /// The host's parent window, as a native handle, while open.
    parent: AtomicUsize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WindowState {
    pub fn new(size: (u32, u32), min_size: (u32, u32)) -> Arc<Self> {
        Arc::new(Self {
            size: Mutex::new(size),
            requested: Mutex::new(None),
            full: Mutex::new(None),
            pending: Mutex::new(None),
            min_size,
            open: AtomicBool::new(false),
            parent: AtomicUsize::new(0),
        })
    }

    /// The current logical size, or the requested one while a request is pending.
    pub fn size(&self) -> (u32, u32) {
        lock(&self.requested).unwrap_or_else(|| *lock(&self.size))
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    /// Asks for a new logical size, at least the minimum size.
    pub fn request(&self, (width, height): (u32, u32)) {
        let size = (width.max(self.min_size.0), height.max(self.min_size.1));
        if size != *lock(&self.size) {
            *lock(&self.requested) = Some(size);
        }
    }

    pub fn is_full_screen(&self) -> bool {
        lock(&self.full).is_some()
    }

    /// Whether full screen is possible here.
    pub fn can_full_screen(&self) -> bool {
        screen::SUPPORTED
    }

    /// Enters or leaves full screen with the next frame.
    pub fn toggle_full_screen(&self) {
        *lock(&self.pending) = Some(!self.is_full_screen());
    }

    /// Makes a pending full-screen change. Runs on the GUI thread.
    fn apply_full_screen(&self, pixels_per_point: f32) {
        let Some(enter) = lock(&self.pending).take() else {
            return;
        };
        if enter && !self.is_full_screen() {
            let Some((host, (width, height))) = screen::enter(self.parent.load(Ordering::Relaxed))
            else {
                return;
            };
            let previous = *lock(&self.size);
            *lock(&self.full) = Some(FullScreen {
                size: previous,
                host,
            });
            let scale = pixels_per_point.max(0.5);
            self.request((
                (width as f32 / scale).round() as u32,
                (height as f32 / scale).round() as u32,
            ));
        } else if !enter {
            self.leave_full_screen();
        }
    }

    /// Restores the host window and the earlier editor size.
    fn leave_full_screen(&self) {
        if let Some(full) = lock(&self.full).take() {
            screen::leave(&full.host);
            self.request(full.size);
        }
    }

    /// Keeps the host window covering the monitor, also after the host resized it.
    fn keep_covering(&self) {
        if let Some(full) = &*lock(&self.full) {
            screen::cover(&full.host);
        }
    }
}

impl<'a> PersistentField<'a, StoredWindow> for Arc<WindowState> {
    fn set(&self, new_value: StoredWindow) {
        *lock(&self.size) = new_value.size;
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&StoredWindow) -> R,
    {
        // Store the size to restore, not the full-screen one.
        let size = lock(&self.full)
            .as_ref()
            .map_or_else(|| *lock(&self.size), |full| full.size);
        f(&StoredWindow { size })
    }
}

#[cfg(windows)]
mod screen {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GWL_STYLE, GetAncestor, GetWindowLongPtrW, GetWindowRect, HWND_TOP, IsWindow,
        SWP_FRAMECHANGED, SWP_NOOWNERZORDER, SetWindowLongPtrW, SetWindowPos, WS_CAPTION,
        WS_THICKFRAME,
    };

    pub const SUPPORTED: bool = true;

    /// The host's top-level window as it was before full screen, and the monitor it
    /// covers.
    #[derive(Debug)]
    pub struct Saved {
        window: usize,
        style: isize,
        rect: (i32, i32, i32, i32),
        monitor: (i32, i32, i32, i32),
    }

    /// Takes the top-level window around `parent` full screen. Returns what to restore
    /// and the monitor's size in physical pixels.
    pub fn enter(parent: usize) -> Option<(Saved, (i32, i32))> {
        if parent == 0 {
            return None;
        }
        // SAFETY: The handles come from the host or the system and are only passed back
        // to the system, which checks them; the structures are plain and initialized.
        unsafe {
            let window = GetAncestor(parent as HWND, GA_ROOT);
            if window.is_null() || IsWindow(window) == 0 {
                return None;
            }
            let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null() {
                return None;
            }
            let mut info: MONITORINFO = zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let mut rect: RECT = zeroed();
            GetWindowRect(window, &mut rect);
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            let area = info.rcMonitor;
            let saved = Saved {
                window: window as usize,
                style,
                rect: (rect.left, rect.top, rect.right, rect.bottom),
                monitor: (area.left, area.top, area.right, area.bottom),
            };
            SetWindowLongPtrW(
                window,
                GWL_STYLE,
                style & !((WS_CAPTION | WS_THICKFRAME) as isize),
            );
            cover(&saved);
            Some((saved, (area.right - area.left, area.bottom - area.top)))
        }
    }

    /// Places the window over the whole monitor, if it is anywhere else.
    pub fn cover(saved: &Saved) {
        let (left, top, right, bottom) = saved.monitor;
        // SAFETY: As in `enter`.
        let current = unsafe {
            let mut rect: RECT = zeroed();
            GetWindowRect(saved.window as HWND, &mut rect);
            (rect.left, rect.top, rect.right, rect.bottom)
        };
        if current != saved.monitor {
            place(saved.window, (left, top, right - left, bottom - top));
        }
    }

    /// Restores the window's frame, place and size.
    pub fn leave(saved: &Saved) {
        let window = saved.window as HWND;
        // SAFETY: As in `enter`; a window closed meanwhile fails the check.
        unsafe {
            if IsWindow(window) == 0 {
                return;
            }
            SetWindowLongPtrW(window, GWL_STYLE, saved.style);
        }
        let (left, top, right, bottom) = saved.rect;
        place(saved.window, (left, top, right - left, bottom - top));
    }

    fn place(window: usize, (x, y, width, height): (i32, i32, i32, i32)) {
        // SAFETY: As in `enter`.
        unsafe {
            let window = window as HWND;
            if IsWindow(window) != 0 {
                SetWindowPos(
                    window,
                    HWND_TOP,
                    x,
                    y,
                    width,
                    height,
                    SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
                );
            }
        }
    }
}

#[cfg(not(windows))]
mod screen {
    pub const SUPPORTED: bool = false;

    /// Nothing to restore on other platforms.
    #[derive(Debug)]
    pub struct Saved;

    pub fn enter(_parent: usize) -> Option<(Saved, (i32, i32))> {
        None
    }

    pub fn cover(_saved: &Saved) {}

    pub fn leave(_saved: &Saved) {}
}

/// Adapts NIH-plug's parent handle to the `raw-window-handle` version baseview uses.
struct ParentAdapter(ParentWindowHandle);

// SAFETY: The handles are the host's, valid while the editor is open.
unsafe impl HasRawWindowHandle for ParentAdapter {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

/// Draws one frame; called with the egui context.
pub type DrawFn = dyn Fn(&Context) + Send + Sync;

/// An NIH-plug editor that opens an egui window and handles resize requests.
pub struct EguiEditor {
    window: Arc<WindowState>,
    title: &'static str,
    setup: Arc<dyn Fn(&Context) + Send + Sync>,
    draw: Arc<DrawFn>,
    /// The host's scale factor, if it set one.
    scale: Mutex<Option<f32>>,
}

impl EguiEditor {
    pub fn new(
        window: Arc<WindowState>,
        title: &'static str,
        setup: impl Fn(&Context) + Send + Sync + 'static,
        draw: impl Fn(&Context) + Send + Sync + 'static,
    ) -> Self {
        Self {
            window,
            title,
            setup: Arc::new(setup),
            draw: Arc::new(draw),
            scale: Mutex::new(None),
        }
    }
}

/// Keeps the window open; dropping it closes the window.
struct Handle {
    window: Arc<WindowState>,
    handle: WindowHandle,
}

// SAFETY: The window handle holds raw pointers that are only used by the host's GUI
// thread, which also drops the editor handle.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // The host may keep its window for the next editor: give it its frame back.
        if let Some(full) = lock(&self.window.full).take() {
            screen::leave(&full.host);
            *lock(&self.window.size) = full.size;
        }
        *lock(&self.window.requested) = None;
        *lock(&self.window.pending) = None;
        self.window.open.store(false, Ordering::Release);
        self.window.parent.store(0, Ordering::Relaxed);
        self.handle.close();
    }
}

impl Editor for EguiEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn Any + Send> {
        let native = match parent {
            ParentWindowHandle::Win32Hwnd(hwnd) => hwnd as usize,
            _ => 0,
        };
        self.window.parent.store(native, Ordering::Relaxed);
        let (width, height) = *lock(&self.window.size);
        let scale = *lock(&self.scale);
        let setup = Arc::clone(&self.setup);
        let draw = Arc::clone(&self.draw);
        let window = Arc::clone(&self.window);
        let handle = EguiWindow::open_parented(
            &ParentAdapter(parent),
            WindowOpenOptions {
                title: String::from(self.title),
                size: Size::new(f64::from(width), f64::from(height)),
                scale: scale.map_or(WindowScalePolicy::SystemScaleFactor, |factor| {
                    WindowScalePolicy::ScaleFactor(f64::from(factor))
                }),
                gl_config: Some(GlConfig {
                    version: (3, 2),
                    red_bits: 8,
                    blue_bits: 8,
                    green_bits: 8,
                    alpha_bits: 8,
                    depth_bits: 24,
                    stencil_bits: 8,
                    samples: None,
                    srgb: true,
                    double_buffer: true,
                    vsync: true,
                    ..GlConfig::default()
                }),
            },
            GraphicsConfig::default(),
            (),
            move |context, _queue, _state| setup(context),
            move |egui_context, queue, _state| {
                window.apply_full_screen(egui_context.pixels_per_point());
                let pending = *lock(&window.requested);
                if let Some(size) = pending {
                    // The host asks the editor for its size while handling the request,
                    // so the request stays pending until it returns.
                    let accepted = context.request_resize();
                    *lock(&window.requested) = None;
                    if accepted {
                        queue.resize(PhySize::new(size.0, size.1));
                        egui_context.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                            size.0 as f32,
                            size.1 as f32,
                        )));
                        *lock(&window.size) = size;
                    }
                }
                // Hosts may resize their window later, and add their frame back.
                window.keep_covering();
                egui_context.request_repaint();
                draw(egui_context);
            },
        );
        self.window.open.store(true, Ordering::Release);
        Box::new(Handle {
            window: Arc::clone(&self.window),
            handle,
        })
    }

    fn size(&self) -> (u32, u32) {
        self.window.size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // The scale cannot change while the window is open.
        if self.window.is_open() {
            return false;
        }
        *lock(&self.scale) = Some(factor);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}

    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use nih_plug::params::persist::PersistentField;

    use super::{StoredWindow, WindowState};

    #[test]
    fn requests_respect_the_minimum_and_persist_the_restored_size() {
        let window = WindowState::new((1_000, 700), (900, 600));
        window.request((400, 2_000));
        assert_eq!(
            window.size(),
            (900, 2_000),
            "pending requests report their size"
        );
        window.set(StoredWindow { size: (1_200, 800) });
        let stored = window.map(|stored| *stored);
        assert_eq!(stored.size, (1_200, 800));
        assert_eq!(
            serde_json::to_string(&stored).expect("serializes"),
            r#"{"size":[1200,800]}"#,
            "the format nih_plug_egui stored"
        );
        window.toggle_full_screen();
        window.apply_full_screen(1.0);
        assert!(
            !window.is_full_screen(),
            "no full screen without a host window"
        );
    }
}
