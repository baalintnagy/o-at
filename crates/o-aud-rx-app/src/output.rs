//! Audio output through cpal, with switchable devices.
//!
//! cpal streams cannot move between threads on every platform, so a controller thread
//! owns the stream and rebuilds it when the device changes or fails. Each stream gets a
//! fresh receiver from the link.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Host, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig,
};
use o_aud_rx::engine::Receiver;
use o_aud_rx::link::RxLink;

use crate::settings::AppSettings;

/// Largest block processed at once; longer device callbacks are split, in frames.
const MAX_BLOCK_FRAMES: usize = 8_192;
/// Delay before reopening a failed device.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// What the output is doing, for the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputStatus {
    /// Output device names.
    pub devices: Vec<String>,
    /// The device in use.
    pub device: Option<String>,
    /// Device rate in Hz and channel count, zero without a device.
    pub sample_rate: u32,
    pub channels: u16,
    pub error: Option<String>,
}

enum Command {
    /// Opens the device named in the settings.
    Reopen,
    RefreshDevices,
    Stop,
}

/// Owns the controller thread; dropping it closes the stream.
pub struct Output {
    commands: Sender<Command>,
    status: Arc<Mutex<OutputStatus>>,
    worker: Option<JoinHandle<()>>,
}

impl Output {
    pub fn start(link: Arc<RxLink>, settings: Arc<AppSettings>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(OutputStatus::default()));
        let worker_status = Arc::clone(&status);
        let worker = thread::Builder::new()
            .name(String::from("o-aud-rx output"))
            .spawn(move || {
                let mut controller = Controller {
                    host: cpal::default_host(),
                    link,
                    settings,
                    status: worker_status,
                    stream: None,
                    failed: Arc::new(AtomicBool::new(false)),
                };
                controller.refresh_devices();
                controller.open();
                loop {
                    let timeout = if controller.stream.is_some() {
                        Duration::from_millis(250)
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
            worker,
        }
    }

    /// Switches to the device stored in the settings.
    pub fn reopen(&self) {
        let _ = self.commands.send(Command::Reopen);
    }

    pub fn refresh_devices(&self) {
        let _ = self.commands.send(Command::RefreshDevices);
    }

    pub fn status(&self) -> OutputStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Controller {
    host: Host,
    link: Arc<RxLink>,
    settings: Arc<AppSettings>,
    status: Arc<Mutex<OutputStatus>>,
    stream: Option<Stream>,
    /// Set by the stream's error callback, for example when the device disappears.
    failed: Arc<AtomicBool>,
}

impl Controller {
    fn update_status(&self, update: impl FnOnce(&mut OutputStatus)) {
        update(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn refresh_devices(&self) {
        let devices: Vec<String> = self
            .host
            .output_devices()
            .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
            .unwrap_or_default();
        self.update_status(|status| status.devices = devices);
    }

    /// Reopens after a stream error, or retries a device that failed to open.
    fn check(&mut self) {
        if self.failed.swap(false, Ordering::Relaxed) || self.stream.is_none() {
            self.refresh_devices();
            self.open();
        }
    }

    fn find_device(&self, name: Option<&str>) -> Option<Device> {
        match name {
            Some(name) => self
                .host
                .output_devices()
                .ok()?
                .find(|device| device.name().is_ok_and(|candidate| candidate == name)),
            None => self.host.default_output_device(),
        }
    }

    fn open(&mut self) {
        // Close the previous stream and its receiver before connecting a new one.
        self.stream = None;
        self.link.set_device_rate(0);
        let requested = self.settings.device();
        let result = self
            .find_device(requested.as_deref())
            .ok_or_else(|| match &requested {
                Some(name) => format!("“{name}” is not available"),
                None => String::from("No output device"),
            })
            .and_then(|device| self.build(&device).map(|stream| (device, stream)));
        match result {
            Ok((device, (stream, config))) => {
                let name = device.name().ok();
                self.stream = Some(stream);
                self.update_status(|status| {
                    status.device = name;
                    status.sample_rate = config.sample_rate.0;
                    status.channels = config.channels;
                    status.error = None;
                });
            }
            Err(error) => self.update_status(|status| {
                status.device = None;
                status.sample_rate = 0;
                status.channels = 0;
                status.error = Some(error);
            }),
        }
    }

    fn build(&self, device: &Device) -> Result<(Stream, StreamConfig), String> {
        let supported = device
            .default_output_config()
            .map_err(|error| error.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let mut receiver = self.link.replace_receiver();
        receiver.set_device_rate(config.sample_rate.0);
        self.link.set_device_rate(config.sample_rate.0);
        let render = Render::new(receiver, Arc::clone(&self.settings), config.channels);
        let stream = match format {
            SampleFormat::F32 => self.stream::<f32>(device, &config, render),
            SampleFormat::I16 => self.stream::<i16>(device, &config, render),
            SampleFormat::U16 => self.stream::<u16>(device, &config, render),
            SampleFormat::I32 => self.stream::<i32>(device, &config, render),
            other => Err(format!("Unsupported sample format {other}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        Ok((stream, config))
    }

    fn stream<T>(
        &self,
        device: &Device,
        config: &StreamConfig,
        mut render: Render,
    ) -> Result<Stream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let failed = Arc::clone(&self.failed);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &OutputCallbackInfo| render.fill(data),
                move |_| failed.store(true, Ordering::Relaxed),
                None,
            )
            .map_err(|error| error.to_string())
    }
}

/// The audio callback's state: the receiver and planar scratch buffers.
struct Render {
    receiver: Receiver,
    settings: Arc<AppSettings>,
    channels: usize,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Render {
    fn new(receiver: Receiver, settings: Arc<AppSettings>, channels: u16) -> Self {
        Self {
            receiver,
            settings,
            channels: usize::from(channels.max(1)),
            left: vec![0.0; MAX_BLOCK_FRAMES],
            right: vec![0.0; MAX_BLOCK_FRAMES],
        }
    }

    /// Fills an interleaved device buffer. Real-time safe.
    fn fill<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T]) {
        let settings = self.settings.receiver_settings();
        let channels = self.channels;
        for block in data.chunks_mut(MAX_BLOCK_FRAMES * channels) {
            let frames = block.len() / channels;
            let (left, right) = (&mut self.left[..frames], &mut self.right[..frames]);
            if channels == 1 {
                self.receiver.process(&mut [&mut *left], &settings);
            } else {
                self.receiver
                    .process(&mut [&mut *left, &mut *right], &settings);
            }
            for (frame, samples) in block.chunks_mut(channels).enumerate() {
                for (channel, sample) in samples.iter_mut().enumerate() {
                    let value = match channel {
                        0 => left[frame],
                        1 => right[frame],
                        _ => 0.0,
                    };
                    *sample = T::from_sample(value);
                }
            }
        }
    }
}
