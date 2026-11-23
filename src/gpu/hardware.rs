use std::{
    num::NonZeroU64,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use crate::{
    ErrorKind, Frame, PixelFormat,
    vision::{Measurements, sample_dimensions},
};

const PARAMS_SIZE: usize = 5 * std::mem::size_of::<u32>();
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    failure: Arc<Mutex<Option<String>>>,
    workspace: Option<Workspace>,
    shared_buffers: bool,
    #[cfg(test)]
    pub(super) panic_on_readback: bool,
}

struct Workspace {
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: Option<wgpu::Buffer>,
    bind_group: wgpu::BindGroup,
}

impl Workspace {
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::ComputePipeline,
        input_size: u64,
        output_size: u64,
        shared: bool,
    ) -> Self {
        use wgpu::BufferUsages as Use;
        let input_usage = Use::STORAGE
            | if shared {
                Use::MAP_WRITE
            } else {
                Use::COPY_DST
            };
        let output_usage = Use::STORAGE | if shared { Use::MAP_READ } else { Use::COPY_SRC };
        let input = buffer(
            device,
            "TileTrail host frame",
            input_size,
            input_usage,
            shared,
        );
        let output = buffer(
            device,
            "TileTrail measurements",
            output_size,
            output_usage,
            false,
        );
        let readback = (!shared).then(|| {
            buffer(
                device,
                "TileTrail measurement readback",
                output_size,
                Use::COPY_DST | Use::MAP_READ,
                false,
            )
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        Self {
            input,
            output,
            readback,
            bind_group,
        }
    }

    fn fits(&self, input_size: u64, output_size: u64) -> bool {
        self.input.size() >= input_size && self.output.size() >= output_size
    }
}

impl Gpu {
    pub(super) fn new() -> Result<Self, String> {
        pollster::block_on(Self::initialize(cfg!(all(
            target_os = "macos",
            target_arch = "aarch64"
        ))))
    }

    async fn initialize(prefer_shared_buffers: bool) -> Result<Self, String> {
        let backends = if cfg!(target_os = "macos") {
            wgpu::Backends::METAL
        } else {
            wgpu::Backends::VULKAN
        };
        if !wgpu::Instance::enabled_backend_features().intersects(backends) {
            return Err("TileTrail has no GPU backend for this target".into());
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .ok_or_else(|| format!("no usable {backends:?} GPU adapter"))?;
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return Err("wgpu selected a software adapter".into());
        }
        let shared_buffers = prefer_shared_buffers
            && adapter
                .features()
                .contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
        let required_features = if shared_buffers {
            wgpu::Features::MAPPABLE_PRIMARY_BUFFERS
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("TileTrail"),
                    required_features,
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(|error| format!("GPU request failed: {error}"))?;
        let failure = Arc::new(Mutex::new(None));
        let device_loss = Arc::clone(&failure);
        device.set_device_lost_callback(move |reason, message| {
            *device_loss.lock().unwrap() = Some(format!("GPU device lost ({reason:?}): {message}"));
        });
        let uncaptured_error = Arc::clone(&failure);
        device.on_uncaptured_error(Box::new(move |error| {
            uncaptured_error
                .lock()
                .unwrap()
                .get_or_insert_with(|| format!("GPU error: {error}"));
        }));
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::include_wgsl!("measure.wgsl"));
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("TileTrail sample measurements"),
            layout: None,
            module: &shader,
            entry_point: Some("measure"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = device.pop_error_scope().await {
            return Err(format!("GPU pipeline initialization failed: {error}"));
        }
        let gpu = Self {
            device,
            queue,
            pipeline,
            failure,
            workspace: None,
            shared_buffers,
            #[cfg(test)]
            panic_on_readback: false,
        };
        gpu.check_device()?;
        Ok(gpu)
    }

    pub(super) fn measure(
        &mut self,
        frame: &Frame<'_>,
    ) -> Result<Measurements, (ErrorKind, String)> {
        self.check_device().map_err(device_lost)?;
        let (width, height) = sample_dimensions(frame.width, frame.height);
        let format = match frame.format {
            PixelFormat::Gray8 => 1,
            PixelFormat::Rgb24 => 3,
            PixelFormat::Rgba8 => 4,
            PixelFormat::Bgra8 => 5,
        };
        // Take only scanlines the nearest-center sampler can access. This
        // retains contiguous host copies while avoiding upload of skipped
        // rows and decoder padding. WGSL still selects and converts pixels.
        let row_bytes = frame.width as usize * frame.format.bytes_per_pixel();
        let upload_limit = MAX_UPLOAD_BYTES.min(
            (self.device.limits().max_storage_buffer_binding_size as usize)
                .saturating_sub(PARAMS_SIZE),
        );
        let upload_size = row_bytes
            .checked_mul(height as usize)
            .filter(|size| *size <= upload_limit)
            .map(|size| size.next_multiple_of(4));
        // Coordinates use integer arithmetic in WGSL.
        let max_input_width = u32::MAX / (2 * width);
        let Some(upload_size) = upload_size.filter(|_| frame.width <= max_input_width) else {
            return Err((
                ErrorKind::GpuUnavailable,
                "frame exceeds the GPU measurement upload or dimension limit".into(),
            ));
        };

        let output_size = u64::from(width) * u64::from(height) * 4;
        let upload_size = (PARAMS_SIZE + upload_size) as u64;
        if !self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.fits(upload_size, output_size))
        {
            // A wide input can require more upload space while a square
            // sample grid requires more output space. Keep both high-water
            // marks so alternating shapes do not reallocate on every push.
            let (input_capacity, output_capacity) =
                self.workspace
                    .take()
                    .map_or((upload_size, output_size), |previous| {
                        (
                            upload_size.max(previous.input.size()),
                            output_size.max(previous.output.size()),
                        )
                    });
            self.workspace = Some(Workspace::new(
                &self.device,
                &self.pipeline,
                input_capacity,
                output_capacity,
                self.shared_buffers,
            ));
        }
        let workspace = self.workspace.as_ref().unwrap();
        let words = [frame.width, row_bytes as u32, format, width, height];
        let populate = |bytes: &mut [u8]| {
            for (destination, word) in bytes[..PARAMS_SIZE].chunks_exact_mut(4).zip(words) {
                destination.copy_from_slice(&word.to_le_bytes());
            }
            let source_size = row_bytes * height as usize;
            let destination = &mut bytes[PARAMS_SIZE..];
            if frame.height == height && frame.stride == row_bytes {
                destination[..source_size].copy_from_slice(&frame.data[..source_size]);
            } else {
                for (y, row) in destination[..source_size]
                    .chunks_exact_mut(row_bytes)
                    .enumerate()
                {
                    let source_row =
                        (2 * y as u64 + 1) * u64::from(frame.height) / (2 * u64::from(height));
                    let offset = source_row as usize * frame.stride;
                    row.copy_from_slice(&frame.data[offset..offset + row_bytes]);
                }
            }
            destination[source_size..].fill(0);
        };
        if self.shared_buffers {
            self.device_call("access mapped GPU input", || {
                populate(&mut workspace.input.slice(..upload_size).get_mapped_range_mut());
            })?;
            workspace.input.unmap();
        } else {
            let mut staging = self
                .queue
                .write_buffer_with(&workspace.input, 0, NonZeroU64::new(upload_size).unwrap())
                .ok_or_else(|| device_lost("could not allocate GPU upload staging".into()))?;
            populate(&mut staging);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &workspace.bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        if let Some(readback) = &workspace.readback {
            encoder.copy_buffer_to_buffer(&workspace.output, 0, readback, 0, output_size);
        }
        self.queue.submit([encoder.finish()]);
        let readback = workspace.readback.as_ref().unwrap_or(&workspace.output);
        let slice = readback.slice(..output_size);
        let (sender, receiver) = mpsc::channel();
        if self.shared_buffers {
            // Remap the directly consumed host buffer for the next frame
            // during the same submission wait as the output readback.
            let ready = sender.clone();
            workspace
                .input
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |result| {
                    let _ = ready.send(result);
                });
        }
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let readiness = self.device_call("wait for GPU readback", || {
            #[cfg(test)]
            assert!(!self.panic_on_readback, "injected GPU readback panic");
            // wgpu 24's Metal blocking wait sleeps in 1 ms increments.
            // Small dispatches generally finish inside that interval. Check
            // for completion briefly before using wgpu's blocking wait.
            let mut remaining = 1 + usize::from(self.shared_buffers);
            if cfg!(target_os = "macos") {
                let deadline = Instant::now() + Duration::from_micros(500);
                while Instant::now() < deadline {
                    self.device.poll(wgpu::Maintain::Poll);
                    while let Ok(result) = receiver.try_recv() {
                        result.map_err(|error| {
                            device_lost(format!("GPU buffer map failed: {error}"))
                        })?;
                        remaining -= 1;
                    }
                    if remaining == 0 {
                        return Ok(());
                    }
                    std::thread::yield_now();
                }
            }
            self.device.poll(wgpu::Maintain::Wait);
            for _ in 0..remaining {
                receiver
                    .try_recv()
                    .map_err(|error| {
                        device_lost(format!("GPU buffer map did not complete: {error}"))
                    })?
                    .map_err(|error| device_lost(format!("GPU buffer map failed: {error}")))?;
            }
            Ok(())
        })?;
        self.check_device().map_err(device_lost)?;
        readiness?;
        let mapped = self.device_call("access GPU readback", || slice.get_mapped_range())?;
        let pixels = mapped
            .chunks_exact(4)
            .map(|rgba| [rgba[0], rgba[1], rgba[2]])
            .collect();
        drop(mapped);
        readback.unmap();
        self.check_device().map_err(device_lost)?;
        Ok(Measurements {
            width,
            height,
            pixels,
            rotation: frame.display.rotation,
        })
    }

    fn check_device(&self) -> Result<(), String> {
        match &*self.failure.lock().unwrap() {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }

    fn device_call<T>(
        &self,
        operation: &str,
        call: impl FnOnce() -> T,
    ) -> Result<T, (ErrorKind, String)> {
        // wgpu 24 reports failures in Device::poll and Buffer::get_mapped_range
        // by panicking, even when the underlying failure is device loss.
        match catch_unwind(AssertUnwindSafe(call)) {
            Ok(result) => Ok(result),
            Err(payload) => {
                let reason = self.check_device().err().unwrap_or_else(|| {
                    let detail = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown GPU error");
                    format!("Could not {operation}: {detail}")
                });
                Err(device_lost(reason))
            }
        }
    }

    #[cfg(test)]
    pub(super) fn destroy_device(&self) {
        self.device.destroy();
        self.device.poll(wgpu::Maintain::Wait);
    }
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
    mapped_at_creation: bool,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation,
    })
}

fn device_lost(message: String) -> (ErrorKind, String) {
    (ErrorKind::DeviceLost, message)
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[test]
fn metal_staging_reuses_storage_across_frame_dimensions() {
    use crate::{DisplayGeometry, Timestamp, vision};

    let mut gpu = pollster::block_on(Gpu::initialize(false)).unwrap();
    assert!(!gpu.shared_buffers);
    let mut peak_input = 0;
    let mut peak_output = 0;
    for (width, height) in [
        (479u32, 315u32),
        (13, 9),
        (408, 704),
        (256, 256),
        (479, 315),
    ] {
        let stride = width as usize * 3 + 2;
        let source_size = stride * (height as usize - 1) + width as usize * 3;
        let data: Vec<_> = (0..source_size).map(|idx| (idx % 251) as u8).collect();
        let frame = Frame {
            data: &data,
            width,
            height,
            stride,
            format: PixelFormat::Rgb24,
            timestamp: Timestamp::new(0, 1, 1),
            display: DisplayGeometry::default(),
        };
        assert_eq!(gpu.measure(&frame).unwrap(), vision::measure(&frame));
        let workspace = gpu.workspace.as_ref().unwrap();
        peak_input = peak_input.max(workspace.input.size());
        peak_output = peak_output.max(workspace.output.size());
        assert!(workspace.fits(peak_input, peak_output));
    }
}
