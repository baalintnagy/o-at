//! Audio capture through cpal, from an input device or, by WASAPI loopback, from what an
//! output device plays.
//!
//! cpal streams cannot move between threads on every platform, so a controller thread
//! owns the streams and rebuilds them when the source changes or fails. Each capture
//! stream gets a fresh port from the link.
//!
//! A loopback stream delivers nothing while the device plays nothing, which would stall
//! the bus. A silent output stream on the same device keeps it running.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Host, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat,
    SizedSample, Stream, StreamConfig, SupportedStreamConfig,
};
use o_aud_tx::interleaved::InterleavedSender;
use o_aud_tx::link::TxLink;

use crate::settings::{AppSettings, Source};

/// Delay before reopening a failed device.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// How often a working stream is checked for errors.
const CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// What the capture is doing, for the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureStatus {
    /// Input device names.
    pub inputs: Vec<String>,
    /// Output device names, which can be captured by loopback.
    pub outputs: Vec<String>,
    /// The source in use, with the device name resolved.
    pub active: Option<Source>,
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
    pub fn start(link: Arc<TxLink>, settings: Arc<AppSettings>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(CaptureStatus::default()));
        let block_frames = Arc::new(AtomicU32::new(0));
        let worker_status = Arc::clone(&status);
        let worker_block_frames = Arc::clone(&block_frames);
        let worker = thread::Builder::new()
            .name(String::from("o-aud-tx capture"))
            .spawn(move || {
                let mut controller = Controller {
                    host: cpal::default_host(),
                    link,
                    settings,
                    status: worker_status,
                    block_frames: worker_block_frames,
                    capture: None,
                    keep_alive: None,
                    failed: Arc::new(AtomicBool::new(false)),
                };
                controller.refresh_devices();
                controller.open();
                loop {
                    let timeout = if controller.capture.is_some() {
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
    settings: Arc<AppSettings>,
    status: Arc<Mutex<CaptureStatus>>,
    block_frames: Arc<AtomicU32>,
    capture: Option<Stream>,
    /// Silent output on a loopback source's device.
    keep_alive: Option<Stream>,
    /// Set by the capture stream's error callback, for example when the device
    /// disappears.
    failed: Arc<AtomicBool>,
}

impl Controller {
    fn update_status(&self, update: impl FnOnce(&mut CaptureStatus)) {
        update(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn refresh_devices(&self) {
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
    }

    /// Reopens after a stream error, or retries a source that failed to open.
    fn check(&mut self) {
        if self.failed.swap(false, Ordering::Relaxed) || self.capture.is_none() {
            self.refresh_devices();
            self.open();
        }
    }

    fn find_device(&self, source: &Source) -> Option<Device> {
        let matches = |device: &Device| {
            device
                .name()
                .is_ok_and(|name| Some(name.as_str()) == source.device.as_deref())
        };
        match (&source.device, source.loopback) {
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
        self.block_frames.store(0, Ordering::Relaxed);
        self.link.set_format(0, 0);
        let source = self.settings.source();
        let result = self
            .find_device(&source)
            .ok_or_else(|| match (&source.device, source.loopback) {
                (Some(name), _) => format!("“{name}” is not available"),
                (None, true) => String::from("No output device"),
                (None, false) => String::from("No input device"),
            })
            .and_then(|device| {
                self.build(&device, source.loopback)
                    .map(|built| (device, built))
            });
        match result {
            Ok((device, (stream, config))) => {
                self.capture = Some(stream);
                self.keep_alive = if source.loopback {
                    silent_output(&device)
                } else {
                    None
                };
                let active = Source {
                    loopback: source.loopback,
                    device: device.name().ok(),
                };
                let kept_alive = self.keep_alive.is_some();
                self.update_status(|status| {
                    status.active = Some(active);
                    status.sample_rate = config.sample_rate.0;
                    status.channels = config.channels;
                    status.kept_alive = kept_alive;
                    status.error = None;
                });
            }
            Err(error) => self.update_status(|status| {
                status.active = None;
                status.sample_rate = 0;
                status.channels = 0;
                status.kept_alive = false;
                status.error = Some(error);
            }),
        }
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
        let sender = InterleavedSender::new(
            self.link.replace_port(),
            config.sample_rate.0,
            usize::from(config.channels),
        );
        self.link
            .set_format(config.sample_rate.0, sender.channels());
        let feed = Feed {
            sender,
            settings: Arc::clone(&self.settings),
            block_frames: Arc::clone(&self.block_frames),
            channels: usize::from(config.channels.max(1)),
        };
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
struct Feed {
    sender: InterleavedSender,
    settings: Arc<AppSettings>,
    block_frames: Arc<AtomicU32>,
    /// Device channels, at least one.
    channels: usize,
}

impl Feed {
    /// Sends an interleaved device buffer. Real-time safe.
    fn push<T>(&mut self, data: &[T])
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let frames = (data.len() / self.channels) as u32;
        self.block_frames.store(frames, Ordering::Relaxed);
        self.sender
            .send(data, self.settings.gain(), Sample::to_sample::<f32>);
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
