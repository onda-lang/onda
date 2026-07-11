use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;

pub struct AudioHost {
    inner: cpal::Host,
}

impl Default for AudioHost {
    fn default() -> Self {
        Self {
            inner: cpal::default_host(),
        }
    }
}

pub struct OutputEndpoint {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
}

impl OutputEndpoint {
    pub fn open(
        host: &AudioHost,
        requested_name: Option<&str>,
        sample_rate_hz: u32,
        block_frames: usize,
    ) -> Result<Self, String> {
        let device = find_output_device(&host.inner, requested_name)?;
        let supported = device
            .default_output_config()
            .map_err(|err| format!("failed to query default output config: {err}"))?;
        let config = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: sample_rate_hz,
            buffer_size: cpal::BufferSize::Fixed(block_frames as u32),
        };
        Ok(Self {
            device,
            config,
            sample_format: supported.sample_format(),
        })
    }

    pub fn channels(&self) -> usize {
        usize::from(self.config.channels)
    }

    pub fn build_stream(
        &self,
        source_channels: usize,
        sample_queue: SampleConsumer,
        errors: StreamErrorState,
    ) -> Result<AudioStream, String> {
        let device_channels = self.channels();
        let stream = match self.sample_format {
            cpal::SampleFormat::F32 => build_output_stream::<f32>(
                &self.device,
                self.config,
                device_channels,
                source_channels,
                sample_queue,
                errors,
            ),
            cpal::SampleFormat::I16 => build_output_stream::<i16>(
                &self.device,
                self.config,
                device_channels,
                source_channels,
                sample_queue,
                errors,
            ),
            cpal::SampleFormat::U16 => build_output_stream::<u16>(
                &self.device,
                self.config,
                device_channels,
                source_channels,
                sample_queue,
                errors,
            ),
            other => Err(format!(
                "unsupported output sample format from audio device: {other:?}"
            )),
        }?;
        Ok(AudioStream {
            inner: stream,
            direction: StreamDirection::Output,
        })
    }
}

pub struct InputEndpoint {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
}

impl InputEndpoint {
    pub fn open(
        host: &AudioHost,
        requested_name: Option<&str>,
        sample_rate_hz: u32,
        block_frames: usize,
    ) -> Result<Self, String> {
        let device = find_input_device(&host.inner, requested_name)?;
        let supported = device
            .default_input_config()
            .map_err(|err| format!("failed to query default input config: {err}"))?;
        let config = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: sample_rate_hz,
            buffer_size: cpal::BufferSize::Fixed(block_frames as u32),
        };
        Ok(Self {
            device,
            config,
            sample_format: supported.sample_format(),
        })
    }

    pub fn build_stream(
        &self,
        target_channels: usize,
        input_queue: SampleProducer,
        errors: StreamErrorState,
    ) -> Result<AudioStream, String> {
        let stream = match self.sample_format {
            cpal::SampleFormat::F32 => build_input_stream::<f32>(
                &self.device,
                self.config,
                target_channels,
                input_queue,
                errors,
            ),
            cpal::SampleFormat::I16 => build_input_stream::<i16>(
                &self.device,
                self.config,
                target_channels,
                input_queue,
                errors,
            ),
            cpal::SampleFormat::U16 => build_input_stream::<u16>(
                &self.device,
                self.config,
                target_channels,
                input_queue,
                errors,
            ),
            other => Err(format!(
                "unsupported input sample format from audio device: {other:?}"
            )),
        }?;
        Ok(AudioStream {
            inner: stream,
            direction: StreamDirection::Input,
        })
    }
}

pub struct AudioStream {
    inner: cpal::Stream,
    direction: StreamDirection,
}

enum StreamDirection {
    Input,
    Output,
}

impl AudioStream {
    pub fn play(&self) -> Result<(), String> {
        self.inner.play().map_err(|err| match self.direction {
            StreamDirection::Input => format!("failed to start audio input stream: {err}"),
            StreamDirection::Output => format!("failed to start audio output stream: {err}"),
        })
    }
}

#[derive(Clone, Default)]
pub struct StreamErrorState {
    inner: Arc<StreamErrorStateInner>,
}

#[derive(Default)]
struct StreamErrorStateInner {
    output_failed: AtomicBool,
    input_failed: AtomicBool,
}

impl StreamErrorState {
    pub fn message(&self) -> Option<String> {
        if self.inner.output_failed.load(Ordering::Acquire) {
            Some("audio output stream error".to_owned())
        } else if self.inner.input_failed.load(Ordering::Acquire) {
            Some("audio input stream error".to_owned())
        } else {
            None
        }
    }
}

struct SampleRing {
    capacity: usize,
    mask: usize,
    slots: Box<[UnsafeCell<f32>]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

// SAFETY: SampleRing is only exposed through one non-cloneable producer and
// one non-cloneable consumer, and publication is synchronized by indices.
unsafe impl Send for SampleRing {}
unsafe impl Sync for SampleRing {}

pub struct SampleProducer {
    inner: Arc<SampleRing>,
    _not_sync: PhantomData<Cell<()>>,
}

pub struct SampleConsumer {
    inner: Arc<SampleRing>,
    _not_sync: PhantomData<Cell<()>>,
}

pub fn sample_ring(capacity: usize) -> (SampleProducer, SampleConsumer) {
    let inner = Arc::new(SampleRing::new(capacity));
    (
        SampleProducer {
            inner: Arc::clone(&inner),
            _not_sync: PhantomData,
        },
        SampleConsumer {
            inner,
            _not_sync: PhantomData,
        },
    )
}

impl SampleProducer {
    pub fn push_slice(&self, input: &[f32]) -> usize {
        self.inner.push_slice(input)
    }
}

impl SampleConsumer {
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn pop_slice_aligned(&self, output: &mut [f32], alignment: usize) -> usize {
        output.fill(0.0);
        self.inner
            .consume(output.len(), alignment, |index, sample| {
                output[index] = sample
            })
    }
}

impl SampleRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = std::iter::repeat_with(|| UnsafeCell::new(0.0))
            .take(capacity)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            capacity,
            mask: capacity - 1,
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        let write = self.write_index.load(Ordering::Acquire);
        let read = self.read_index.load(Ordering::Acquire);
        write.saturating_sub(read)
    }

    fn push_slice(&self, input: &[f32]) -> usize {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        let available = self.capacity.saturating_sub(write.saturating_sub(read));
        let count = input.len().min(available);
        for (offset, sample) in input.iter().copied().take(count).enumerate() {
            let index = (write + offset) & self.mask;
            // SAFETY: the single producer only writes unpublished slots.
            unsafe { *self.slots[index].get() = sample };
        }
        if count != 0 {
            self.write_index.store(write + count, Ordering::Release);
        }
        count
    }

    fn produce(
        &self,
        requested: usize,
        alignment: usize,
        mut sample_at: impl FnMut(usize) -> f32,
    ) -> usize {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        let available = self.capacity.saturating_sub(write.saturating_sub(read));
        let mut count = requested.min(available);
        count -= count % alignment.max(1);
        for offset in 0..count {
            let index = (write + offset) & self.mask;
            // SAFETY: the single producer only writes unpublished slots.
            unsafe { *self.slots[index].get() = sample_at(offset) };
        }
        if count != 0 {
            self.write_index.store(write + count, Ordering::Release);
        }
        count
    }

    fn consume(
        &self,
        requested: usize,
        alignment: usize,
        mut consume: impl FnMut(usize, f32),
    ) -> usize {
        let read = self.read_index.load(Ordering::Relaxed);
        let write = self.write_index.load(Ordering::Acquire);
        let mut count = requested.min(write.saturating_sub(read));
        count -= count % alignment.max(1);
        for offset in 0..count {
            let index = (read + offset) & self.mask;
            // SAFETY: the single consumer only reads published slots.
            consume(offset, unsafe { *self.slots[index].get() });
        }
        if count != 0 {
            self.read_index.store(read + count, Ordering::Release);
        }
        count
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    device_channels: usize,
    source_channels: usize,
    sample_queue: SampleConsumer,
    errors: StreamErrorState,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                configure_current_thread_fp_mode();
                write_output_data(data, device_channels, source_channels, &sample_queue.inner);
            },
            move |_err| {
                errors.inner.output_failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|err| format!("failed to build audio output stream: {err}"))
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    target_channels: usize,
    input_queue: SampleProducer,
    errors: StreamErrorState,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let device_channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                configure_current_thread_fp_mode();
                write_input_data(data, device_channels, target_channels, &input_queue.inner);
            },
            move |_err| {
                errors.inner.input_failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|err| format!("failed to build audio input stream: {err}"))
}

fn write_output_data<T>(
    data: &mut [T],
    device_channels: usize,
    source_channels: usize,
    sample_queue: &SampleRing,
) where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    data.fill(T::from_sample(0.0));
    if device_channels == 0 || source_channels == 0 {
        return;
    }
    let frames = data.len() / device_channels;
    sample_queue.consume(
        frames.saturating_mul(source_channels),
        source_channels,
        |index, value| {
            let frame = index / source_channels;
            let source_channel = index % source_channels;
            if source_channels == 1 {
                for sample in &mut data[frame * device_channels..(frame + 1) * device_channels] {
                    *sample = T::from_sample(value);
                }
            } else if source_channel < device_channels {
                data[frame * device_channels + source_channel] = T::from_sample(value);
            }
        },
    );
}

fn write_input_data<T>(
    data: &[T],
    device_channels: usize,
    target_channels: usize,
    input_queue: &SampleRing,
) where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    if device_channels == 0 || target_channels == 0 {
        return;
    }
    let frames = data.len() / device_channels;
    input_queue.produce(
        frames.saturating_mul(target_channels),
        target_channels,
        |index| {
            let frame = index / target_channels;
            let target_channel = index % target_channels;
            if target_channel < device_channels {
                f32::from_sample(data[frame * device_channels + target_channel])
            } else {
                0.0
            }
        },
    );
}

fn find_output_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .output_devices()
            .map_err(|err| format!("failed to enumerate output devices: {err}"))?
            .find(|device| device.to_string() == name)
            .ok_or_else(|| format!("output device '{name}' was not found")),
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output audio device available".to_owned()),
    }
}

fn find_input_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .input_devices()
            .map_err(|err| format!("failed to enumerate input devices: {err}"))?
            .find(|device| device.to_string() == name)
            .ok_or_else(|| format!("input device '{name}' was not found")),
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input audio device available".to_owned()),
    }
}

pub fn available_audio_devices() -> (Vec<String>, Vec<String>) {
    enumerate_devices_silenced(|host| {
        let inputs = host
            .input_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|device| device.to_string())
            .collect();
        let outputs = host
            .output_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|device| device.to_string())
            .collect();
        (inputs, outputs)
    })
}

pub fn input_audio_devices() -> Vec<String> {
    enumerate_devices_silenced(|host| {
        host.input_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|device| device.to_string())
            .collect()
    })
}

pub fn output_audio_devices() -> Vec<String> {
    enumerate_devices_silenced(|host| {
        host.output_devices()
            .ok()
            .into_iter()
            .flatten()
            .map(|device| device.to_string())
            .collect()
    })
}

fn enumerate_devices_silenced<T>(f: impl FnOnce(&cpal::Host) -> T) -> T {
    #[cfg(target_os = "linux")]
    {
        static STDERR_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = STDERR_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let host = cpal::default_host();
        with_stderr_silenced(|| f(&host))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let host = cpal::default_host();
        f(&host)
    }
}

#[cfg(target_os = "linux")]
fn with_stderr_silenced<T>(f: impl FnOnce() -> T) -> T {
    let stderr = std::io::stderr();
    let stderr_fd = stderr.as_raw_fd();
    let null = match std::fs::OpenOptions::new().write(true).open("/dev/null") {
        Ok(file) => file,
        Err(_) => return f(),
    };
    let null_fd = null.as_raw_fd();

    // SAFETY: access is serialized and the original stderr fd is restored.
    unsafe {
        let saved = libc::dup(stderr_fd);
        if saved < 0 {
            return f();
        }
        if libc::dup2(null_fd, stderr_fd) < 0 {
            let _ = libc::close(saved);
            return f();
        }
        let result = f();
        let _ = libc::dup2(saved, stderr_fd);
        let _ = libc::close(saved);
        result
    }
}

thread_local! {
    static REALTIME_FP_MODE_CONFIGURED: Cell<bool> = const { Cell::new(false) };
}

pub fn configure_current_thread_fp_mode() {
    REALTIME_FP_MODE_CONFIGURED.with(|configured| {
        if configured.get() {
            return;
        }
        configure_fp_mode();
        configured.set(true);
    });
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn configure_fp_mode() {
    // Flush denormals to zero to prevent stalls in feedback/smoothing paths.
    unsafe {
        let mut csr = 0_u32;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        let desired = csr | (1 << 15) | (1 << 6);
        if desired != csr {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &desired, options(nostack, preserves_flags));
        }
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn configure_fp_mode() {}

#[cfg(test)]
mod tests {
    use super::{sample_ring, write_input_data, write_output_data};

    #[test]
    fn output_callback_maps_channels_with_bulk_ring_transfer() {
        let (producer, consumer) = sample_ring(8);
        assert_eq!(producer.push_slice(&[1.0, 10.0, 2.0, 20.0]), 4);

        let mut mono = [0.0_f32; 2];
        write_output_data(&mut mono, 1, 2, &consumer.inner);

        assert_eq!(mono, [1.0, 2.0]);
        assert!(consumer.is_empty());
    }

    #[test]
    fn output_callback_only_consumes_complete_source_frames() {
        let (producer, consumer) = sample_ring(8);
        assert_eq!(producer.push_slice(&[1.0, 10.0, 2.0]), 3);

        let mut stereo = [99.0_f32; 4];
        write_output_data(&mut stereo, 2, 2, &consumer.inner);

        assert_eq!(stereo, [1.0, 10.0, 0.0, 0.0]);
        assert_eq!(consumer.len(), 1);
    }

    #[test]
    fn input_callback_zero_fills_missing_target_channels() {
        let (producer, consumer) = sample_ring(8);
        write_input_data(&[1.0_f32, 2.0], 1, 2, &producer.inner);

        let mut captured = [99.0_f32; 4];
        assert_eq!(consumer.pop_slice_aligned(&mut captured, 2), 4);
        assert_eq!(captured, [1.0, 0.0, 2.0, 0.0]);
    }
}
