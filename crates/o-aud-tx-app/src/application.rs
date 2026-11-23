//! Capture of what one application plays, through WASAPI process loopback (Windows 10
//! version 2004 and later).
//!
//! An application is named by its executable. Its audio usually comes from a child
//! process, as in browsers, so the capture targets the topmost process of that
//! executable and includes its whole process tree. Process IDs change when the
//! application restarts; the controller finds the target again.
//!
//! The capture runs on its own thread: process loopback is activated asynchronously and
//! needs a multithreaded COM apartment, while cpal keeps the controller thread
//! single-threaded. While the application is quiet nothing arrives, so the thread sends
//! silence instead (see [`SilenceFill`](o_aud_tx::silence::SilenceFill)).

#[cfg(windows)]
pub use windows_impl::{
    ApplicationCapture, CHANNELS, Target, find, initialize_com, is_running, list,
};

#[cfg(not(windows))]
pub use other::{ApplicationCapture, CHANNELS, Target, find, initialize_com, is_running, list};

/// An application with an audio session, for the source list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    /// The executable name, for example `firefox.exe`.
    pub executable: String,
    /// Whether one of its sessions is playing.
    pub playing: bool,
}

/// Whether the executable name `found` is the one `wanted`, which may omit `.exe`.
/// Windows file names ignore case.
pub fn matches(found: &str, wanted: &str) -> bool {
    found.eq_ignore_ascii_case(wanted)
        || found
            .len()
            .checked_sub(4)
            .and_then(|stem| found.get(..stem).zip(found.get(stem..)))
            .is_some_and(|(stem, extension)| {
                extension.eq_ignore_ascii_case(".exe") && stem.eq_ignore_ascii_case(wanted)
            })
}

#[cfg(windows)]
mod windows_impl {
    use std::collections::{BTreeMap, HashMap};
    use std::mem::{size_of, zeroed};
    use std::process;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, SyncSender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use o_aud_tx::silence::SilenceFill;
    use wasapi::{
        AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType,
        SessionState, StreamMode, WasapiError, WaveFormat, deinitialize, initialize_mta,
        initialize_sta,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };

    use super::{Application, matches};
    use crate::capture::Feed;

    /// Channels captured; process loopback mixes the application to this layout.
    pub const CHANNELS: usize = 2;
    /// Longest wait for a packet before checking for silence and the stop flag, in ms.
    const WAIT_MS: u32 = 10;
    /// Longest wait for the capture thread to open the stream.
    const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
    /// Ancestors followed at most, in case process IDs were reused into a cycle.
    const MAX_DEPTH: usize = 64;

    /// The process whose tree is captured.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Target {
        pub process_id: u32,
        /// The executable name as the system reports it.
        pub executable: String,
    }

    /// Joins the controller thread to COM the way cpal does (single-threaded), for
    /// listing sessions. Call once, before cpal is used on the thread.
    pub fn initialize_com() {
        // A failure, for example an apartment already chosen by cpal, leaves COM usable.
        let _ = initialize_sta();
    }

    struct ProcessEntry {
        parent: u32,
        executable: String,
    }

    /// All running processes by ID.
    fn processes() -> HashMap<u32, ProcessEntry> {
        let mut processes = HashMap::new();
        // SAFETY: The snapshot handle is checked and closed; the entry is a plain struct
        // whose size field is set as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return processes;
            }
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                processes.insert(
                    entry.th32ProcessID,
                    ProcessEntry {
                        parent: entry.th32ParentProcessID,
                        executable: String::from_utf16_lossy(&entry.szExeFile[..length]),
                    },
                );
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        processes
    }

    /// Process IDs of the audio sessions on all output devices, with whether each is
    /// playing. Expired sessions, system sounds and this process are left out.
    fn sessions() -> Vec<(u32, bool)> {
        let Ok(enumerator) = DeviceEnumerator::new() else {
            return Vec::new();
        };
        let Ok(devices) = enumerator.get_device_collection(Direction::Render) else {
            return Vec::new();
        };
        let own = process::id();
        let mut sessions = Vec::new();
        for device in devices.into_iter().flatten() {
            let Ok(enumerator) = device
                .get_iaudiosessionmanager()
                .and_then(|manager| manager.get_audiosessionenumerator())
            else {
                continue;
            };
            let count = enumerator.get_count().unwrap_or(0);
            sessions.extend((0..count).filter_map(|index| {
                let control = enumerator.get_session(index).ok()?;
                let state = control.get_state().ok()?;
                let process_id = control.get_process_id().ok()?;
                (state != SessionState::Expired && process_id != 0 && process_id != own)
                    .then_some((process_id, state == SessionState::Active))
            }));
        }
        sessions
    }

    /// Applications with an audio session, sorted by name.
    pub fn list() -> Vec<Application> {
        let processes = processes();
        let mut applications: BTreeMap<String, Application> = BTreeMap::new();
        for (process_id, playing) in sessions() {
            let Some(process) = processes.get(&process_id) else {
                continue;
            };
            applications
                .entry(process.executable.to_lowercase())
                .and_modify(|application| application.playing |= playing)
                .or_insert_with(|| Application {
                    executable: process.executable.clone(),
                    playing,
                });
        }
        applications.into_values().collect()
    }

    /// The topmost ancestor of `process_id` that runs the same executable.
    fn root(processes: &HashMap<u32, ProcessEntry>, process_id: u32) -> u32 {
        let mut current = process_id;
        for _ in 0..MAX_DEPTH {
            let Some(process) = processes.get(&current) else {
                break;
            };
            let parent_is_same = process.parent != current
                && processes
                    .get(&process.parent)
                    .is_some_and(|parent| parent.executable == process.executable);
            if !parent_is_same {
                break;
            }
            current = process.parent;
        }
        current
    }

    /// The process to capture for `executable`: preferably the root of one that plays,
    /// then of one with a session, then of any running one.
    pub fn find(executable: &str) -> Option<Target> {
        let processes = processes();
        let is_wanted = |process_id: &u32| {
            processes
                .get(process_id)
                .is_some_and(|process| matches(&process.executable, executable))
        };
        let mut sessions = sessions();
        // Playing sessions first.
        sessions.sort_by_key(|&(_, playing)| !playing);
        let process_id = sessions
            .into_iter()
            .map(|(process_id, _)| process_id)
            .find(is_wanted)
            .or_else(|| processes.keys().copied().find(is_wanted))?;
        let root = root(&processes, process_id);
        Some(Target {
            process_id: root,
            executable: processes.get(&root)?.executable.clone(),
        })
    }

    /// Whether `target` still runs, and was not replaced by another program.
    pub fn is_running(target: &Target) -> bool {
        processes()
            .get(&target.process_id)
            .is_some_and(|process| process.executable == target.executable)
    }

    /// Owns the capture thread; dropping it stops the capture and drops its port.
    pub struct ApplicationCapture {
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl ApplicationCapture {
        /// Captures `target` at `sample_rate` Hz in [`CHANNELS`] channels into `feed`,
        /// which must be made for that format. Returns once the stream runs. `failed`
        /// is set if it stops later.
        pub fn start(
            target: &Target,
            sample_rate: u32,
            feed: Feed,
            failed: Arc<AtomicBool>,
        ) -> Result<Self, String> {
            let stop = Arc::new(AtomicBool::new(false));
            let (opened, result) = mpsc::sync_channel(1);
            let process_id = target.process_id;
            let thread_stop = Arc::clone(&stop);
            let worker = thread::Builder::new()
                .name(String::from("o-aud-tx application capture"))
                .spawn(move || {
                    run(
                        process_id,
                        sample_rate,
                        feed,
                        &thread_stop,
                        &failed,
                        &opened,
                    );
                })
                .map_err(|error| error.to_string())?;
            let capture = Self {
                stop,
                worker: Some(worker),
            };
            match result.recv_timeout(OPEN_TIMEOUT) {
                Ok(Ok(())) => Ok(capture),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(String::from("The application capture did not start")),
            }
        }
    }

    impl Drop for ApplicationCapture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// The opened stream and the buffers of its loop, allocated before it runs.
    struct Stream {
        client: AudioClient,
        capture: AudioCaptureClient,
        event: Handle,
        /// Received packets, as native-endian float samples.
        packet: Vec<[u8; 4]>,
    }

    fn open(process_id: u32, sample_rate: u32) -> Result<Stream, WasapiError> {
        let format = WaveFormat::new(
            32,
            32,
            SampleType::Float,
            sample_rate as usize,
            CHANNELS,
            None,
        );
        let mut client = AudioClient::new_application_loopback_client(process_id, true)?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client.initialize_client(&format, Direction::Capture, mode, None)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        // Packets hold about 10 ms; the reported buffer size is not reliable for process
        // loopback, so one second is reserved.
        let packet = vec![[0; 4]; sample_rate as usize * CHANNELS];
        client.start_stream()?;
        Ok(Stream {
            client,
            capture,
            event,
            packet,
        })
    }

    /// The capture thread.
    fn run(
        process_id: u32,
        sample_rate: u32,
        mut feed: Feed,
        stop: &AtomicBool,
        failed: &AtomicBool,
        opened: &SyncSender<Result<(), String>>,
    ) {
        let com = initialize_mta().is_ok();
        boost_priority();
        match open(process_id, sample_rate) {
            Ok(mut stream) => {
                let _ = opened.send(Ok(()));
                if !stream.run(&mut feed, stop) {
                    failed.store(true, Ordering::Relaxed);
                }
                let _ = stream.client.stop_stream();
            }
            Err(error) => {
                let _ = opened.send(Err(format!("Application capture failed: {error}")));
            }
        }
        if com {
            deinitialize();
        }
    }

    impl Stream {
        /// Sends what arrives, and silence while nothing does, until `stop` is set.
        /// Returns false if the stream failed. Real-time safe.
        fn run(&mut self, feed: &mut Feed, stop: &AtomicBool) -> bool {
            let mut fill = SilenceFill::new(feed.sample_rate(), Instant::now());
            while !stop.load(Ordering::Relaxed) {
                // A timeout only means that nothing arrived.
                let _ = self.event.wait_for_event(WAIT_MS);
                loop {
                    match self.capture.get_next_packet_size() {
                        Ok(Some(0) | None) => break,
                        Ok(Some(_)) => {}
                        Err(_) => return false,
                    }
                    match self
                        .capture
                        .read_from_device(self.packet.as_flattened_mut())
                    {
                        Ok((0, _)) => break,
                        Ok((frames, _)) => {
                            let samples = frames as usize * CHANNELS;
                            feed.push_with(&self.packet[..samples], f32::from_ne_bytes);
                            fill.delivered(Instant::now());
                        }
                        // The packet was dropped; the next one fits.
                        Err(WasapiError::DataLengthTooShort { .. }) => {}
                        Err(_) => return false,
                    }
                }
                feed.push_silence(fill.due(Instant::now()));
            }
            true
        }
    }

    /// Raises the capture thread's priority, as cpal does for its device threads.
    fn boost_priority() {
        // SAFETY: Plain Win32 calls on the current thread's pseudo handle.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
    }
}

/// Application capture needs Windows; elsewhere no application is listed or opened.
#[cfg(not(windows))]
mod other {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::Application;
    use crate::capture::Feed;

    pub const CHANNELS: usize = 2;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Target {
        pub process_id: u32,
        pub executable: String,
    }

    pub fn initialize_com() {}

    pub fn list() -> Vec<Application> {
        Vec::new()
    }

    pub fn find(_: &str) -> Option<Target> {
        None
    }

    pub fn is_running(_: &Target) -> bool {
        false
    }

    pub struct ApplicationCapture;

    impl ApplicationCapture {
        pub fn start(_: &Target, _: u32, _: Feed, _: Arc<AtomicBool>) -> Result<Self, String> {
            Err(String::from("Application capture needs Windows"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn executable_names_match_without_case_or_extension() {
        assert!(matches("firefox.exe", "firefox.exe"));
        assert!(matches("Firefox.EXE", "firefox.exe"));
        assert!(matches("firefox.exe", "Firefox"));
        assert!(!matches("firefox.exe", "fire"));
        assert!(!matches("firefox.exe", "chrome.exe"));
        assert!(!matches("exe", "e"));
    }
}
