//! Audio capture through cpal, from an input device or, by WASAPI loopback, from what an
//! output device plays; or, by process loopback, from what one application plays (see
//! [`application`](crate::application)).
//!
//! cpal streams cannot move between threads on every platform, so a controller thread
//! owns the streams and rebuilds them when the source changes or fails. Each capture
//! stream gets a fresh port from the link.
//!
//! A loopback stream delivers nothing while the device plays nothing, which would stall
//! the bus. A silent output stream on the same device keeps it running. An application
//! capture sends silence of its own instead.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Host, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat,
    SizedSample, Stream, StreamConfig, SupportedStreamConfig,
};
use o_aud_tx::interleaved::InterleavedSender;
use o_aud_tx::link::TxLink;

use crate::application::{self, Application, ApplicationCapture, CHANNELS, Target};
use crate::settings::{RouteSettings, Source};

/// Delay before reopening a failed device.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// How often a working stream is checked for errors.
const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How often the applications with audio are listed again, and a captured application is
/// checked for having quit.
const APPLICATION_INTERVAL: Duration = Duration::from_secs(2);
/// Rate in Hz of an application capture when the default output's rate is unknown.
const FALLBACK_RATE: u32 = 48_000;

/// What the capture is doing, for the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureStatus {
    /// Input device names.
    pub inputs: Vec<String>,
    /// Output device names, which can be captured by loopback.
    pub outputs: Vec<String>,
    /// Applications with audio sessions, which can be captured by process loopback.
    pub applications: Vec<Application>,
    /// The source in use, with the device or executable name resolved.
    pub active: Option<Source>,
    /// The captured process, whose child processes are captured too.
    pub process_id: Option<u32>,
    /// Device rate in Hz and channel count, zero without a device.
    pub sample_rate: u32,
    pub channels: u16,
    /// Whether a loopback source is kept running by a silent output stream.
    pub kept_alive: bool,
    pub error: Option<String>,
}

enum Command {
    /// Opens the source stored in the settings.
    Reopen,
    RefreshDevices,
    Stop,
}

/// Owns the controller thread; dropping it closes the streams.
pub struct Capture {
    commands: Sender<Command>,
    status: Arc<Mutex<CaptureStatus>>,
    /// Frames of the last device buffer.
    block_frames: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(link: Arc<TxLink>, settings: Arc<RouteSettings>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(CaptureStatus::default()));
        let block_frames = Arc::new(AtomicU32::new(0));
        let worker_status = Arc::clone(&status);
        let worker_block_frames = Arc::clone(&block_frames);
        let worker = thread::Builder::new()
            .name(String::from("o-aud-tx capture"))
            .spawn(move || {
                // Before cpal, which uses the same apartment.
                application::initialize_com();
                let mut controller = Controller {
                    host: cpal::default_host(),
                    link,
                    settings,
                    status: worker_status,
                    block_frames: worker_block_frames,
                    capture: None,
                    keep_alive: None,
                    application: None,
                    target: None,
                    listed: Instant::now(),
                    failed: Arc::new(AtomicBool::new(false)),
                };
                controller.refresh_devices();
                controller.open();
                loop {
                    let timeout = if controller.is_open() {
                        CHECK_INTERVAL
                    } else {
                        RETRY_INTERVAL
                    };
                    match receiver.recv_timeout(timeout) {
                        Ok(Command::Reopen) => controller.open(),
                        Ok(Command::RefreshDevices) => controller.refresh_devices(),
                        Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => controller.check(),
                    }
                }
            })
            .ok();
        Self {
            commands,
            status,
            block_frames,
            worker,
        }
    }

    /// Switches to the source stored in the settings.
    pub fn reopen(&self) {
        let _ = self.commands.send(Command::Reopen);
    }

    pub fn refresh_devices(&self) {
        let _ = self.commands.send(Command::RefreshDevices);
    }

    pub fn status(&self) -> CaptureStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Frames of the last device buffer, zero before the first.
    pub fn block_frames(&self) -> u32 {
        self.block_frames.load(Ordering::Relaxed)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Controller {
    host: Host,
    link: Arc<TxLink>,
    settings: Arc<RouteSettings>,
    status: Arc<Mutex<CaptureStatus>>,
    block_frames: Arc<AtomicU32>,
    capture: Option<Stream>,
    /// Silent output on a loopback source's device.
    keep_alive: Option<Stream>,
    /// The capture of an application source, instead of `capture`.
    application: Option<ApplicationCapture>,
    /// The process that `application` captures.
    target: Option<Target>,
    /// When the applications were last listed.
    listed: Instant,
    /// Set by the capture stream's error callback, for example when the device
    /// disappears, or when the application capture stops.
    failed: Arc<AtomicBool>,
}

/// A source that opened.
struct Opened {
    active: Source,
    sample_rate: u32,
    channels: u16,
    kept_alive: bool,
    process_id: Option<u32>,
}

impl Controller {
    fn update_status(&self, update: impl FnOnce(&mut CaptureStatus)) {
        update(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn refresh_devices(&mut self) {
        let inputs: Vec<String> = self
            .host
            .input_devices()
            .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
            .unwrap_or_default();
        let outputs: Vec<String> = self
            .host
            .output_devices()
            .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
            .unwrap_or_default();
        self.update_status(|status| {
            status.inputs = inputs;
            status.outputs = outputs;
        });
        self.refresh_applications();
    }

    fn refresh_applications(&mut self) {
        self.listed = Instant::now();
        let applications = application::list();
        self.update_status(|status| status.applications = applications);
    }

    fn is_open(&self) -> bool {
        self.capture.is_some() || self.application.is_some()
    }

    /// Reopens after a stream error or when the captured application quit, or retries a
    /// source that failed to open.
    fn check(&mut self) {
        if self.listed.elapsed() >= APPLICATION_INTERVAL {
            self.refresh_applications();
            if self
                .target
                .as_ref()
                .is_some_and(|target| !application::is_running(target))
            {
                self.failed.store(true, Ordering::Relaxed);
            }
        }
        if self.failed.swap(false, Ordering::Relaxed) || !self.is_open() {
            self.refresh_devices();
            self.open();
        }
    }

    /// The device of an input or loopback source.
    fn find_device(&self, device: Option<&str>, loopback: bool) -> Option<Device> {
        let matches = |found: &Device| found.name().is_ok_and(|name| Some(name.as_str()) == device);
        match (device, loopback) {
            (Some(_), true) => self.host.output_devices().ok()?.find(matches),
            (Some(_), false) => self.host.input_devices().ok()?.find(matches),
            (None, true) => self.host.default_output_device(),
            (None, false) => self.host.default_input_device(),
        }
    }

    fn open(&mut self) {
        // Close the previous streams and their port before a new port takes the bus.
        self.capture = None;
        self.keep_alive = None;
        self.application = None;
        self.target = None;
        self.block_frames.store(0, Ordering::Relaxed);
        self.link.set_format(0, 0);
        let result = match self.settings.source() {
            Source::Input(device) => self.open_device(device.as_deref(), false),
            Source::Loopback(device) => self.open_device(device.as_deref(), true),
            Source::Application(executable) => self.open_application(&executable),
        };
        match result {
            Ok(opened) => self.update_status(|status| {
                status.active = Some(opened.active);
                status.sample_rate = opened.sample_rate;
                status.channels = opened.channels;
                status.kept_alive = opened.kept_alive;
                status.process_id = opened.process_id;
                status.error = None;
            }),
            Err(error) => {
                self.link.set_format(0, 0);
                self.update_status(|status| {
                    status.active = None;
                    status.sample_rate = 0;
                    status.channels = 0;
                    status.kept_alive = false;
                    status.process_id = None;
                    status.error = Some(error);
                });
            }
        }
    }

    /// Opens an input device, or an output device by loopback.
    fn open_device(&mut self, device: Option<&str>, loopback: bool) -> Result<Opened, String> {
        let found = self
            .find_device(device, loopback)
            .ok_or_else(|| match (device, loopback) {
                (Some(name), _) => format!("“{name}” is not available"),
                (None, true) => String::from("No output device"),
                (None, false) => String::from("No input device"),
            })?;
        let (stream, config) = self.build(&found, loopback)?;
        self.capture = Some(stream);
        self.keep_alive = if loopback {
            silent_output(&found)
        } else {
            None
        };
        let name = found.name().ok();
        Ok(Opened {
            active: if loopback {
                Source::Loopback(name)
            } else {
                Source::Input(name)
            },
            sample_rate: config.sample_rate.0,
            channels: config.channels,
            kept_alive: self.keep_alive.is_some(),
            process_id: None,
        })
    }

    /// Opens the capture of an application by its executable name, at the default
    /// output's rate, so that the system does not resample twice.
    fn open_application(&mut self, executable: &str) -> Result<Opened, String> {
        let target = application::find(executable)
            .ok_or_else(|| format!("“{executable}” is not running"))?;
        let sample_rate = self
            .host
            .default_output_device()
            .and_then(|device| device.default_output_config().ok())
            .map_or(FALLBACK_RATE, |config| config.sample_rate().0);
        let feed = self.feed(sample_rate, CHANNELS);
        let capture =
            ApplicationCapture::start(&target, sample_rate, feed, Arc::clone(&self.failed))?;
        self.application = Some(capture);
        self.target = Some(target.clone());
        Ok(Opened {
            active: Source::Application(target.executable),
            sample_rate,
            channels: CHANNELS as u16,
            kept_alive: false,
            process_id: Some(target.process_id),
        })
    }

    /// Takes a fresh port for a stream at `sample_rate` Hz with `channels` interleaved
    /// channels, and tells the link the format.
    fn feed(&self, sample_rate: u32, channels: usize) -> Feed {
        Feed::new(
            &self.link,
            Arc::clone(&self.settings),
            Arc::clone(&self.block_frames),
            sample_rate,
            channels,
        )
    }

    /// Builds and starts the capture stream; `loopback` captures an output device.
    fn build(&self, device: &Device, loopback: bool) -> Result<(Stream, StreamConfig), String> {
        let supported = if loopback {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(|error| error.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let feed = self.feed(config.sample_rate.0, usize::from(config.channels));
        let stream = match format {
            SampleFormat::F32 => self.stream::<f32>(device, &config, feed),
            SampleFormat::I16 => self.stream::<i16>(device, &config, feed),
            SampleFormat::U16 => self.stream::<u16>(device, &config, feed),
            SampleFormat::I32 => self.stream::<i32>(device, &config, feed),
            other => Err(format!("Unsupported sample format {other}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        Ok((stream, config))
    }

    fn stream<T>(
        &self,
        device: &Device,
        config: &StreamConfig,
        mut feed: Feed,
    ) -> Result<Stream, String>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let failed = Arc::clone(&self.failed);
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &InputCallbackInfo| feed.push(data),
                move |_| failed.store(true, Ordering::Relaxed),
                None,
            )
            .map_err(|error| error.to_string())
    }
}

/// The capture callback's state.
pub struct Feed {
    sender: InterleavedSender,
    settings: Arc<RouteSettings>,
    block_frames: Arc<AtomicU32>,
    /// Device channels, at least one.
    channels: usize,
    /// Stream rate in Hz.
    sample_rate: u32,
}

impl Feed {
    /// Takes a fresh port from `link` for a stream at `sample_rate` Hz with `channels`
    /// interleaved channels, and tells the link the format.
    fn new(
        link: &TxLink,
        settings: Arc<RouteSettings>,
        block_frames: Arc<AtomicU32>,
        sample_rate: u32,
        channels: usize,
    ) -> Self {
        let sender = InterleavedSender::new(link.replace_port(), sample_rate, channels);
        link.set_format(sample_rate, sender.channels());
        Self {
            sender,
            settings,
            block_frames,
            channels: channels.max(1),
            sample_rate,
        }
    }

    /// Stream rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Sends an interleaved device buffer. Real-time safe.
    fn push<T>(&mut self, data: &[T])
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        self.push_with(data, Sample::to_sample::<f32>);
    }

    /// Sends an interleaved buffer, converting samples with `to_f32`. Real-time safe.
    pub fn push_with<T: Copy>(&mut self, data: &[T], to_f32: impl Fn(T) -> f32) {
        let frames = (data.len() / self.channels) as u32;
        self.block_frames.store(frames, Ordering::Relaxed);
        self.sender.send(data, self.settings.gain(), to_f32);
    }

    /// Sends `frames` frames of silence. Real-time safe.
    pub fn push_silence(&mut self, frames: u64) {
        self.sender.send_silence(frames, self.settings.gain());
    }
}

/// Opens a silent output stream on `device`, so its loopback capture keeps delivering
/// while nothing else plays. Best effort: without it, capture pauses during silence.
fn silent_output(device: &Device) -> Option<Stream> {
    let supported: SupportedStreamConfig = device.default_output_config().ok()?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::F32 => silent_stream::<f32>(device, &config),
        SampleFormat::I16 => silent_stream::<i16>(device, &config),
        SampleFormat::U16 => silent_stream::<u16>(device, &config),
        SampleFormat::I32 => silent_stream::<i32>(device, &config),
        _ => None,
    }?;
    stream.play().ok()?;
    Some(stream)
}

fn silent_stream<T: SizedSample>(device: &Device, config: &StreamConfig) -> Option<Stream> {
    device
        .build_output_stream(
            config,
            |data: &mut [T], _: &OutputCallbackInfo| data.fill(T::EQUILIBRIUM),
            |_| {},
            None,
        )
        .ok()
}
