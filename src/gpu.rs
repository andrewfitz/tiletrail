//! Measurement backend selection and frame-local recovery.

use crate::{
    AnalysisError, Backend, BackendDiagnostic, BackendMode, ErrorKind, Frame, Timestamp, vision,
};

#[cfg(feature = "gpu")]
mod hardware;

enum Engine {
    Cpu,
    #[cfg(feature = "gpu")]
    Gpu(Box<hardware::Gpu>),
    #[cfg(test)]
    FakeGpu,
    Failed(AnalysisError),
}

pub(crate) struct BackendSession {
    mode: BackendMode,
    engine: Engine,
    diagnostics: Vec<BackendDiagnostic>,
    #[cfg(test)]
    inject_loss: bool,
}

impl BackendSession {
    pub(crate) fn new(mode: BackendMode, start: Timestamp) -> Result<Self, AnalysisError> {
        let validated_target = cfg!(all(target_os = "macos", target_arch = "aarch64"));
        let (engine, reason) = match mode {
            BackendMode::CpuOnly => (Engine::Cpu, None),
            BackendMode::Auto if !validated_target => (
                Engine::Cpu,
                Some("automatic GPU selection is only enabled on Apple Silicon macOS".into()),
            ),
            _ => match open_gpu() {
                Ok(engine) => (
                    engine,
                    (!validated_target).then(|| "GPU target has not been validated".into()),
                ),
                Err(message) if matches!(mode, BackendMode::GpuRequired) => {
                    return Err(AnalysisError {
                        kind: ErrorKind::GpuUnavailable,
                        message,
                        resume_at: start,
                    });
                }
                Err(message) => (Engine::Cpu, Some(message)),
            },
        };
        Ok(Self::with_engine(mode, start, engine, reason))
    }

    fn with_engine(
        mode: BackendMode,
        start: Timestamp,
        engine: Engine,
        reason: Option<String>,
    ) -> Self {
        let mut session = Self {
            mode,
            engine,
            diagnostics: Vec::new(),
            #[cfg(test)]
            inject_loss: false,
        };
        session.diagnostics.push(BackendDiagnostic {
            timestamp: start,
            backend: session.backend(),
            reason,
        });
        session
    }

    pub(crate) fn measure(
        &mut self,
        frame: &Frame<'_>,
    ) -> Result<vision::Measurements, AnalysisError> {
        let result: Result<_, AnalysisError> = match &mut self.engine {
            Engine::Cpu => Ok(vision::measure(frame)),
            #[cfg(feature = "gpu")]
            Engine::Gpu(gpu) => gpu.measure(frame).map_err(|(kind, message)| AnalysisError {
                kind,
                message,
                resume_at: frame.timestamp,
            }),
            #[cfg(test)]
            Engine::FakeGpu => Ok(vision::measure(frame)),
            Engine::Failed(error) => return Err(error.clone()),
        };
        #[cfg(test)]
        let result = if self.inject_loss && !matches!(self.engine, Engine::Cpu) {
            self.inject_loss = false;
            Err(AnalysisError {
                kind: ErrorKind::DeviceLost,
                message: "injected GPU device loss".into(),
                resume_at: frame.timestamp,
            })
        } else {
            result
        };
        match result {
            Ok(measurements) => Ok(measurements),
            Err(error) if matches!(self.mode, BackendMode::Auto) => {
                self.engine = Engine::Cpu;
                self.diagnostics.push(BackendDiagnostic {
                    timestamp: frame.timestamp,
                    backend: Backend::Cpu,
                    reason: Some(error.message),
                });
                // The caller still owns this host-readable frame, and the
                // tracker has not consumed measurements for it yet.
                Ok(vision::measure(frame))
            }
            Err(error) => {
                self.diagnostics.push(BackendDiagnostic {
                    timestamp: frame.timestamp,
                    backend: Backend::Gpu,
                    reason: Some(error.message.clone()),
                });
                self.engine = Engine::Failed(error.clone());
                Err(error)
            }
        }
    }

    pub(crate) fn backend(&self) -> Backend {
        match self.engine {
            Engine::Cpu => Backend::Cpu,
            #[cfg(feature = "gpu")]
            Engine::Gpu(_) => Backend::Gpu,
            #[cfg(test)]
            Engine::FakeGpu => Backend::Gpu,
            Engine::Failed(_) => Backend::Gpu,
        }
    }

    pub(crate) fn diagnostics(&self) -> &[BackendDiagnostic] {
        &self.diagnostics
    }

    #[cfg(test)]
    pub(crate) fn fake_gpu(mode: BackendMode, start: Timestamp) -> Self {
        Self::with_engine(mode, start, Engine::FakeGpu, None)
    }

    #[cfg(test)]
    pub(crate) fn inject_device_loss(&mut self) {
        self.inject_loss = true;
    }
}

#[cfg(feature = "gpu")]
fn open_gpu() -> Result<Engine, String> {
    hardware::Gpu::new().map(|gpu| Engine::Gpu(Box::new(gpu)))
}

#[cfg(not(feature = "gpu"))]
fn open_gpu() -> Result<Engine, String> {
    Err("TileTrail was built without the gpu feature".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplayGeometry, PixelFormat, Rotation};

    #[cfg(not(feature = "gpu"))]
    #[test]
    fn unavailable_gpu_obeys_switch_at_scan_start() {
        let start = Timestamp::new(-9, 1, 30000);
        let auto = BackendSession::new(BackendMode::Auto, start).unwrap();
        assert_eq!(auto.backend(), Backend::Cpu);
        assert_eq!(auto.diagnostics()[0].timestamp, start);
        assert!(auto.diagnostics()[0].reason.is_some());
        let error = BackendSession::new(BackendMode::GpuRequired, start)
            .err()
            .unwrap();
        assert_eq!(error.kind, ErrorKind::GpuUnavailable);
        assert_eq!(error.resume_at, start);
    }

    #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn metal_measurements_match_cpu_and_recover_from_device_destruction() {
        let start = Timestamp::new(-3, 1, 30000);
        let mut gpu = BackendSession::new(BackendMode::Auto, start).unwrap();
        assert_eq!(gpu.backend(), Backend::Gpu, "{:?}", gpu.diagnostics());
        for format in [
            PixelFormat::Gray8,
            PixelFormat::Rgb24,
            PixelFormat::Rgba8,
            PixelFormat::Bgra8,
        ] {
            let width = 321;
            let height = 157;
            let stride = width * format.bytes_per_pixel() + 3;
            let source_size = (height - 1) * stride + width * format.bytes_per_pixel();
            let data: Vec<_> = (0..source_size)
                .map(|index| ((index * 17 + index / 7) % 251) as u8)
                .collect();
            let frame = Frame {
                data: &data,
                width: width as u32,
                height: height as u32,
                stride,
                format,
                timestamp: start,
                display: DisplayGeometry {
                    rotation: Rotation::R270,
                    ..Default::default()
                },
            };
            let cpu = vision::measure(&frame);
            let measured = gpu.measure(&frame).unwrap();
            assert_eq!(gpu.backend(), Backend::Gpu);
            assert_eq!((cpu.width, cpu.height), (measured.width, measured.height));
            assert_eq!(cpu.rotation, measured.rotation);
            assert_eq!(cpu.pixels, measured.pixels);
        }

        let next = Timestamp::new(10, 1, 30000);
        let frame = Frame {
            data: &[99],
            width: 1,
            height: 1,
            stride: 1,
            format: PixelFormat::Gray8,
            timestamp: next,
            display: DisplayGeometry::default(),
        };
        let Engine::Gpu(device) = &gpu.engine else {
            unreachable!()
        };
        device.destroy_device();
        assert_eq!(gpu.measure(&frame).unwrap().pixels, [[99, 99, 99]]);
        assert_eq!(gpu.backend(), Backend::Cpu);
        assert_eq!(gpu.diagnostics()[1].timestamp, next);
        assert!(
            gpu.diagnostics()[1]
                .reason
                .as_ref()
                .unwrap()
                .contains("device lost")
        );

        let mut required = BackendSession::new(BackendMode::GpuRequired, start).unwrap();
        let Engine::Gpu(device) = &required.engine else {
            unreachable!()
        };
        device.destroy_device();
        let first_error = required.measure(&frame).err().unwrap();
        assert_eq!(first_error.kind, ErrorKind::DeviceLost);
        assert_eq!(first_error.resume_at, next);
        let later = Frame {
            timestamp: Timestamp::new(12, 1, 30000),
            ..frame
        };
        assert_eq!(required.measure(&later).err().unwrap().resume_at, next);

        let mut readback = BackendSession::new(BackendMode::Auto, start).unwrap();
        let Engine::Gpu(device) = &mut readback.engine else {
            unreachable!()
        };
        device.panic_on_readback = true;
        assert_eq!(readback.measure(&frame).unwrap().pixels, [[99, 99, 99]]);
        assert_eq!(readback.backend(), Backend::Cpu);
        assert_eq!(readback.diagnostics()[1].timestamp, next);
        assert!(
            readback.diagnostics()[1]
                .reason
                .as_ref()
                .unwrap()
                .contains("injected GPU readback panic")
        );
    }

    #[test]
    fn fake_device_loss_transitions_once_and_replays_current_frame() {
        let start = Timestamp::new(1, 1, 24);
        let next = Timestamp::new(2, 1, 24);
        let mut session = BackendSession::fake_gpu(BackendMode::Auto, start);
        let frame = Frame {
            data: &[8],
            width: 1,
            height: 1,
            stride: 1,
            format: PixelFormat::Gray8,
            timestamp: next,
            display: DisplayGeometry {
                rotation: Rotation::R90,
                ..Default::default()
            },
        };
        session.inject_device_loss();
        assert_eq!(session.measure(&frame).unwrap().pixels, [[8, 8, 8]]);
        assert_eq!(session.backend(), Backend::Cpu);
        assert_eq!(session.diagnostics()[1].timestamp, next);
        assert_eq!(session.measure(&frame).unwrap().pixels, [[8, 8, 8]]);
        assert_eq!(session.diagnostics().len(), 2);
    }
}
