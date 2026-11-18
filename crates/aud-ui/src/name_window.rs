//! A separate window for entering a name.
//!
//! Plugin hosts such as Ableton Live keep keystrokes for their own shortcuts, so text
//! typed into a plugin editor may never arrive. A top-level window with its own thread,
//! and therefore its own message loop, receives the keyboard directly in any host.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use baseview::gl::GlConfig;
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui::{Align2, CentralPanel, Color32, Context, Frame, Id, Key, Rect, Ui, pos2, vec2};
use egui_baseview::{EguiWindow, GraphicsConfig, Queue};
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;

use crate::theme::{self, BACKGROUND, TEXT_DIM, font};
use crate::widgets::{button, select_all, text_field};

/// Logical window size in pixels.
const WIDTH: f32 = 380.0;
const HEIGHT: f32 = 136.0;
const MARGIN: f32 = 16.0;

#[derive(Default)]
struct Shared {
    open: AtomicBool,
    close_requested: AtomicBool,
    result: Mutex<Option<String>>,
}

/// Opens name windows and collects what was entered. Dropping it closes an open window
/// and waits for its thread, so the window never outlives the editor (or the plugin
/// library) that opened it.
#[derive(Default)]
pub struct NameWindow {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl NameWindow {
    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::Relaxed)
    }

    /// Opens a window titled `title` with `initial` selected for replacing. Does nothing
    /// while a window is open.
    pub fn open(&mut self, title: &str, initial: &str, hint: &str, color: Color32) {
        if self.is_open() {
            return;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.shared.close_requested.store(false, Ordering::Relaxed);
        self.shared.open.store(true, Ordering::Relaxed);
        let dialog = Dialog {
            shared: Arc::clone(&self.shared),
            text: initial.to_owned(),
            hint: hint.to_owned(),
            color,
            first_frame: true,
        };
        let options = WindowOpenOptions {
            title: title.to_owned(),
            size: Size::new(f64::from(WIDTH), f64::from(HEIGHT)),
            scale: WindowScalePolicy::ScaleFactor(system_scale()),
            gl_config: Some(GlConfig::default()),
        };
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name(String::from("name window"))
            .spawn(move || {
                EguiWindow::open_blocking(
                    options,
                    GraphicsConfig::default(),
                    dialog,
                    |context, _, _| theme::apply(context),
                    |context, queue, dialog| dialog.update(context, queue),
                );
                shared.open.store(false, Ordering::Relaxed);
            });
        match spawned {
            Ok(worker) => self.worker = Some(worker),
            Err(_) => self.shared.open.store(false, Ordering::Relaxed),
        }
    }

    /// The name confirmed in the window, once.
    pub fn take(&self) -> Option<String> {
        self.shared
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl Drop for NameWindow {
    fn drop(&mut self) {
        self.shared.close_requested.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Dialog {
    shared: Arc<Shared>,
    text: String,
    hint: String,
    color: Color32,
    first_frame: bool,
}

impl Dialog {
    fn update(&mut self, context: &Context, queue: &mut Queue) {
        if self.shared.close_requested.load(Ordering::Relaxed) {
            queue.close_window();
            return;
        }
        let mut confirmed = false;
        let mut cancelled = false;
        CentralPanel::default()
            .frame(Frame::NONE.fill(BACKGROUND))
            .show(context, |ui| {
                let rect = ui.max_rect().shrink(MARGIN);
                ui.painter().text(
                    rect.left_top() + vec2(2.0, 6.0),
                    Align2::LEFT_CENTER,
                    "Bus name: up to 32 bytes, case-insensitive",
                    font(11.0),
                    TEXT_DIM,
                );
                let field = Rect::from_min_size(
                    rect.left_top() + vec2(0.0, 18.0),
                    vec2(rect.width(), 30.0),
                );
                let key = "name-window";
                let response = text_field(ui, field, key, &mut self.text, &self.hint, self.color);
                if self.first_frame {
                    response.request_focus();
                    select_all(ui.ctx(), Id::new(("text", key)), self.text.chars().count());
                    self.first_frame = false;
                }
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(Key::Enter),
                        input.key_pressed(Key::Escape),
                    )
                });
                let ok = Rect::from_min_size(
                    pos2(rect.right() - 168.0, rect.bottom() - 26.0),
                    vec2(80.0, 26.0),
                );
                let cancel = Rect::from_min_size(
                    pos2(rect.right() - 80.0, rect.bottom() - 26.0),
                    vec2(80.0, 26.0),
                );
                let ok_clicked =
                    button(ui, Id::new("name-ok"), ok, "OK", self.color, true).clicked();
                let cancel_clicked = button(
                    ui,
                    Id::new("name-cancel"),
                    cancel,
                    "CANCEL",
                    self.color,
                    false,
                )
                .clicked();
                confirmed = ok_clicked || enter;
                cancelled = cancel_clicked || escape;
            });
        if confirmed {
            *self
                .shared
                .result
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(self.text.trim().to_owned());
            queue.close_window();
        } else if cancelled {
            queue.close_window();
        }
    }
}

/// Width of the EDIT button beside a name field, in logical pixels.
const EDIT_WIDTH: f32 = 52.0;

/// A name field with an EDIT button that opens a [`NameWindow`], for hosts that keep
/// keystrokes from plugin editors. `draft` holds the text between frames and follows
/// `current` while the field is not being edited. Returns a name confirmed inline (by
/// Enter or leaving the field) or in the window.
pub fn name_editor(
    ui: &mut Ui,
    rect: Rect,
    (draft, current): (&mut String, &str),
    window: &mut NameWindow,
    (key, title): (&str, &str),
    color: Color32,
) -> Option<String> {
    let input = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - EDIT_WIDTH - 6.0, rect.bottom()),
    );
    let edit = Rect::from_min_max(pos2(input.right() + 6.0, rect.top()), rect.max);
    if !ui.memory(|memory| memory.has_focus(Id::new(("text", key)))) {
        current.clone_into(draft);
    }
    let mut confirmed = None;
    if text_field(ui, input, key, draft, "Bus name", color).lost_focus() && draft != current {
        confirmed = Some(draft.clone());
    }
    let open = window.is_open();
    if button(ui, Id::new(("edit-name", key)), edit, "EDIT", color, open)
        .on_hover_text("Edit the name in its own window, if the host keeps keystrokes")
        .clicked()
    {
        window.open(title, current, "Bus name", color);
    }
    if let Some(name) = window.take() {
        name.clone_into(draft);
        confirmed = Some(name);
    }
    confirmed
}

/// The display scale factor for windows opened by this process. An explicit factor is
/// used because egui-baseview keeps its initial viewport size when baseview applies the
/// system factor after opening. Hosts that are not DPI aware get 1.0 and Windows scales
/// the window.
#[cfg(windows)]
pub fn system_scale() -> f64 {
    // SAFETY: A plain Win32 query without arguments.
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 }
}

/// The display scale factor for windows opened by this process.
#[cfg(not(windows))]
pub fn system_scale() -> f64 {
    1.0
}
