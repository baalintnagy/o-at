//! Shows an executable's own icon on its window.
//!
//! baseview registers its window class without an icon, so title bars and taskbar
//! buttons would show a generic one. Executables embed their icon as resource 1 (see
//! their build scripts); this sets it on the window once it exists. Plugins must not
//! call it: the process, and its icon, belong to the host.

#[cfg(windows)]
mod platform {
    use std::iter::once;
    use std::ptr::null;
    use std::thread;
    use std::time::Duration;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetSystemMetrics, GetWindowThreadProcessId, ICON_BIG, ICON_SMALL, IMAGE_ICON,
        LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW,
        WM_SETICON,
    };

    /// Resource ID of the embedded application icon.
    const ICON_RESOURCE: usize = 1;
    /// How long to wait for the window to appear.
    const ATTEMPTS: usize = 100;
    const INTERVAL: Duration = Duration::from_millis(100);

    pub fn apply_when_open(title: &str) {
        let title: Vec<u16> = title.encode_utf16().chain(once(0)).collect();
        let _ = thread::Builder::new()
            .name(String::from("window icon"))
            .spawn(move || {
                for _ in 0..ATTEMPTS {
                    if apply(&title) {
                        return;
                    }
                    thread::sleep(INTERVAL);
                }
            });
    }

    /// Sets the icon on this process's window titled `title`, if it exists.
    fn apply(title: &[u16]) -> bool {
        // SAFETY: `title` is NUL-terminated and outlives the calls; the handles come from
        // the system and are only passed back to it.
        unsafe {
            let window = FindWindowW(null(), title.as_ptr());
            if window.is_null() {
                return false;
            }
            let mut process = 0;
            GetWindowThreadProcessId(window, &mut process);
            if process != GetCurrentProcessId() {
                return false;
            }
            let module = GetModuleHandleW(null());
            for (kind, width, height) in [
                (ICON_BIG, SM_CXICON, SM_CYICON),
                (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
            ] {
                let icon = LoadImageW(
                    module,
                    ICON_RESOURCE as *const u16,
                    IMAGE_ICON,
                    GetSystemMetrics(width),
                    GetSystemMetrics(height),
                    LR_DEFAULTCOLOR,
                );
                if !icon.is_null() {
                    SendMessageW(window, WM_SETICON, kind as usize, icon as isize);
                }
            }
            true
        }
    }
}

/// Sets the executable's embedded icon on its window titled `title` once the window
/// opens. Returns at once; a background thread waits for the window for up to 10 s.
#[cfg(windows)]
pub fn apply_when_open(title: &str) {
    platform::apply_when_open(title);
}

/// Does nothing: other platforms take the icon from the application bundle or desktop
/// entry.
#[cfg(not(windows))]
pub fn apply_when_open(_title: &str) {}
