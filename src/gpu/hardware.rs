use std::sync::{Arc, Mutex, mpsc};

use crate::{
    ErrorKind, Frame, PixelFormat,
    vision::{Measurements, sample_dimensions},
};
use wgpu::util::DeviceExt;

const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    failure: Arc<Mutex<Option<String>>>,
}

impl Gpu {
    pub(super) fn new() -> Result<Self, String> {
        pollster::block_on(Self::initialize())
    }

    async fn initialize() -> Result<Self, String> {
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
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("TileTrail"),
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
        };
        gpu.check_device()?;
        Ok(gpu)
    }

    pub(super) fn measure(&self, frame: &Frame<'_>) -> Result<Measurements, (ErrorKind, String)> {
        self.check_device().map_err(device_lost)?;
        let (width, height) = sample_dimensions(frame.width, frame.height);
        let format = match frame.format {
            PixelFormat::Gray8 => 1,
            PixelFormat::Rgb24 => 3,
            PixelFormat::Rgba8 => 4,
            PixelFormat::Bgra8 => 5,
        };
        let pixel_bytes = frame.format.bytes_per_pixel();
        // Input is checked by the analyzer. Ignore unrelated trailing bytes.
        let source_size =
            (frame.height as usize - 1) * frame.stride + frame.width as usize * pixel_bytes;
        let upload_size = source_size.next_multiple_of(4);
        let upload_limit =
            MAX_UPLOAD_BYTES.min(self.device.limits().max_storage_buffer_binding_size as usize);
        // Coordinates use integer arithmetic in WGSL, and this cap ensures
        // that even an extremely wide or tall host frame cannot overflow it.
        let max_input_axis = u32::MAX / (2 * width.max(height));
        if upload_size > upload_limit
            || frame.width.max(frame.height) > max_input_axis
            || frame.stride > u32::MAX as usize
        {
            return Err((
                ErrorKind::GpuUnavailable,
                "frame exceeds the GPU measurement upload or dimension limit".into(),
            ));
        }

        let input = self.buffer(
            "TileTrail host frame",
            upload_size as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let source = &frame.data[..source_size];
        let whole_words = source_size / 4 * 4;
        if whole_words > 0 {
            self.queue.write_buffer(&input, 0, &source[..whole_words]);
        }
        if upload_size > whole_words {
            let mut tail = [0u8; 4];
            tail[..source_size - whole_words].copy_from_slice(&source[whole_words..]);
            self.queue.write_buffer(&input, whole_words as u64, &tail);
        }

        let words = [
            frame.width,
            frame.height,
            frame.stride as u32,
            format,
            width,
            height,
        ];
        let params = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("TileTrail frame dimensions"),
                contents: &words.map(u32::to_le_bytes).concat(),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output_size = u64::from(width) * u64::from(height) * 4;
        let output = self.buffer(
            "TileTrail measurements",
            output_size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = self.buffer(
            "TileTrail measurement readback",
            output_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_size);
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        self.check_device().map_err(device_lost)?;
        receiver
            .try_recv()
            .map_err(|error| device_lost(format!("GPU readback did not complete: {error}")))?
            .map_err(|error| device_lost(format!("GPU readback failed: {error}")))?;
        let mapped = readback.slice(..).get_mapped_range();
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

    fn buffer(&self, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    fn check_device(&self) -> Result<(), String> {
        match &*self.failure.lock().unwrap() {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(super) fn destroy_device(&self) {
        self.device.destroy();
        self.device.poll(wgpu::Maintain::Wait);
    }
}

fn device_lost(message: String) -> (ErrorKind, String) {
    (ErrorKind::DeviceLost, message)
}
