//! Implementation of the ABI declared in include/tiletrail.h.

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::{ptr, slice};

use crate::{
    AnalysisError, AnalysisResult, Analyzer, AnalyzerConfig, BackendMode, DisplayGeometry,
    ErrorKind, FeedLayout, Frame, MarkerRule, PixelFormat, RESULT_SCHEMA_VERSION, Rotation,
    Sampling, TimeBase, Timestamp,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CTime {
    pts: i64,
    numerator: u32,
    denominator: u32,
}

impl From<CTime> for Timestamp {
    fn from(time: CTime) -> Self {
        Self {
            pts: time.pts,
            time_base: TimeBase {
                numerator: time.numerator,
                denominator: time.denominator,
            },
        }
    }
}

impl From<Timestamp> for CTime {
    fn from(time: Timestamp) -> Self {
        Self {
            pts: time.pts,
            numerator: time.time_base.numerator,
            denominator: time.time_base.denominator,
        }
    }
}

#[repr(C)]
struct CConfig {
    mode: u32,
    sampling: u32,
}

#[repr(C)]
struct CFrame {
    data: *const u8,
    len: u64,
    stride: u64,
    width: u32,
    height: u32,
    format: u32,
    rotation: u32,
    pixel_aspect_num: u32,
    pixel_aspect_den: u32,
    timestamp: CTime,
}

#[repr(C)]
struct CBuffer {
    data: *const u8,
    len: u64,
}

impl CBuffer {
    const EMPTY: Self = Self {
        data: ptr::null(),
        len: 0,
    };
}

#[repr(C)]
struct CError {
    code: i32,
    has_resume: u32,
    resume_at: CTime,
    message: [c_char; 256],
}

struct AnalyzerHandle {
    analyzer: Analyzer,
    panicked: bool,
}

#[derive(Debug, Default)]
struct Failure {
    code: i32,
    resume_at: Option<Timestamp>,
    message: String,
}

impl Failure {
    fn argument(message: &str) -> Self {
        Self {
            code: 6,
            message: message.to_owned(),
            ..Self::default()
        }
    }

    fn panic() -> Self {
        Self {
            code: 7,
            message: "TileTrail panicked".to_owned(),
            ..Self::default()
        }
    }

    fn to_error(&self) -> CError {
        let mut message = [0; 256];
        let mut len = self.message.len().min(message.len() - 1);
        while !self.message.is_char_boundary(len) {
            len -= 1;
        }
        for (out, byte) in message.iter_mut().zip(&self.message.as_bytes()[..len]) {
            *out = *byte as c_char;
        }
        CError {
            code: self.code,
            has_resume: u32::from(self.resume_at.is_some()),
            resume_at: self.resume_at.map(CTime::from).unwrap_or_default(),
            message,
        }
    }
}

impl From<AnalysisError> for Failure {
    fn from(error: AnalysisError) -> Self {
        let code = match error.kind {
            ErrorKind::InvalidFrame => 1,
            ErrorKind::InvalidTimestamp => 2,
            ErrorKind::InvalidRange => 3,
            ErrorKind::GpuUnavailable => 4,
            ErrorKind::DeviceLost => 5,
        };
        Self {
            code,
            resume_at: Some(error.resume_at),
            message: error.message,
        }
    }
}

fn catch<T>(call: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(value) => Some(value),
        Err(payload) => {
            // Dropping an arbitrary panic payload could itself panic.
            std::mem::forget(payload);
            None
        }
    }
}

fn boundary(error: *mut CError, call: impl FnOnce() -> Result<(), Failure>) -> i32 {
    let failure = match catch(call) {
        Some(Ok(())) => Failure::default(),
        Some(Err(failure)) => failure,
        None => Failure::panic(),
    };
    if !error.is_null() {
        // SAFETY: The C caller supplies writable, aligned error storage.
        unsafe { error.write(failure.to_error()) };
    }
    failure.code
}

#[unsafe(no_mangle)]
extern "C" fn tiletrail_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_create(
    config: *const CConfig,
    start: CTime,
    out: *mut *mut AnalyzerHandle,
    error: *mut CError,
) -> i32 {
    create_analyzer(config, start, FeedLayout::Meeting, out, error)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_create_feeds(
    config: *const CConfig,
    start: CTime,
    columns: u32,
    rows: u32,
    out: *mut *mut AnalyzerHandle,
    error: *mut CError,
) -> i32 {
    let layout = match (columns, rows) {
        (0, 0) => FeedLayout::Automatic,
        _ => FeedLayout::Grid { columns, rows },
    };
    create_analyzer(config, start, layout, out, error)
}

fn create_analyzer(
    config: *const CConfig,
    start: CTime,
    layout: FeedLayout,
    out: *mut *mut AnalyzerHandle,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if out.is_null() {
            return Err(Failure::argument("missing analyzer output"));
        }
        // SAFETY: The caller supplies a writable output and an optional readable config.
        unsafe { out.write(ptr::null_mut()) };
        let mut options = AnalyzerConfig::default();
        if let Some(config) = unsafe { config.as_ref() } {
            options.mode = match config.mode {
                0 => BackendMode::Auto,
                1 => BackendMode::CpuOnly,
                2 => BackendMode::GpuRequired,
                _ => return Err(Failure::argument("unknown backend mode")),
            };
            options.sampling = match config.sampling {
                0 => Sampling::EveryFrame,
                1 => Sampling::Sparse,
                _ => return Err(Failure::argument("unknown sampling policy")),
            };
        }
        let analyzer = Analyzer::with_layout(options, start.into(), layout)?;
        let handle = Box::new(AnalyzerHandle {
            analyzer,
            panicked: false,
        });
        unsafe { out.write(Box::into_raw(handle)) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_push(
    analyzer: *mut AnalyzerHandle,
    frame: *const CFrame,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        // SAFETY: The caller guarantees serialized access and valid storage.
        let handle =
            unsafe { analyzer.as_mut() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let frame = unsafe { frame.as_ref() }.ok_or_else(|| Failure::argument("missing frame"))?;
        let format = match frame.format {
            0 => PixelFormat::Gray8,
            1 => PixelFormat::Rgb24,
            2 => PixelFormat::Rgba8,
            3 => PixelFormat::Bgra8,
            _ => return Err(Failure::argument("unknown pixel format")),
        };
        let rotation = match frame.rotation {
            0 => Rotation::R0,
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => return Err(Failure::argument("unknown display rotation")),
        };
        let len = usize::try_from(frame.len)
            .ok()
            .filter(|len| *len <= isize::MAX as usize)
            .ok_or_else(|| Failure::argument("buffer length exceeds addressable range"))?;
        let stride = usize::try_from(frame.stride)
            .map_err(|_| Failure::argument("row stride exceeds addressable range"))?;
        let data = if len == 0 {
            &[]
        } else {
            if frame.data.is_null() {
                return Err(Failure::argument("missing pixel buffer"));
            }
            // SAFETY: The caller supplies len readable bytes, valid until return.
            unsafe { slice::from_raw_parts(frame.data, len) }
        };
        handle.panicked = true;
        let result = handle.analyzer.push(Frame {
            data,
            width: frame.width,
            height: frame.height,
            stride,
            format,
            timestamp: frame.timestamp.into(),
            display: DisplayGeometry {
                rotation,
                pixel_aspect_num: frame.pixel_aspect_num,
                pixel_aspect_den: frame.pixel_aspect_den,
            },
        });
        handle.panicked = false;
        result.map_err(Failure::from)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_result_json(
    analyzer: *const AnalyzerHandle,
    end: CTime,
    out: *mut CBuffer,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if out.is_null() {
            return Err(Failure::argument("missing result output"));
        }
        // SAFETY: The caller supplies writable output and a valid analyzer.
        unsafe { out.write(CBuffer::EMPTY) };
        let handle =
            unsafe { analyzer.as_ref() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let result = handle.analyzer.snapshot(end.into())?;
        // SAFETY: out points to writable storage exclusively borrowed for the call.
        unsafe { write_json(out, &result) }
    })
}

unsafe fn write_json(out: *mut CBuffer, value: &impl serde::Serialize) -> Result<(), Failure> {
    let data = serde_json::to_vec(value).map_err(|error| Failure {
        code: 8,
        message: error.to_string(),
        ..Failure::default()
    })?;
    let data = data.into_boxed_slice();
    let len = data.len() as u64;
    let raw = Box::into_raw(data);
    // SAFETY: out points to writable storage and does not own an allocation.
    unsafe {
        out.write(CBuffer {
            data: raw.cast::<u8>(),
            len,
        });
    }
    Ok(())
}

unsafe fn parse_json<T: serde::de::DeserializeOwned>(
    data: *const u8,
    len: u64,
) -> Result<T, Failure> {
    let len = usize::try_from(len)
        .ok()
        .filter(|len| *len > 0 && *len <= isize::MAX as usize)
        .ok_or_else(|| Failure::argument("JSON input length is out of range"))?;
    if data.is_null() {
        return Err(Failure::argument("missing JSON input"));
    }
    // SAFETY: The caller supplies len readable bytes for the duration of the call.
    let bytes = unsafe { slice::from_raw_parts(data, len) };
    serde_json::from_slice(bytes).map_err(|error| Failure::argument(&error.to_string()))
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_scene_markers_json(
    analysis_json: *const u8,
    analysis_len: u64,
    rule_json: *const u8,
    rule_len: u64,
    out: *mut CBuffer,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if out.is_null() {
            return Err(Failure::argument("missing result output"));
        }
        // SAFETY: out is writable and each input contains the declared readable bytes.
        unsafe { out.write(CBuffer::EMPTY) };
        let analysis: AnalysisResult = unsafe { parse_json(analysis_json, analysis_len) }?;
        if analysis.schema_version != RESULT_SCHEMA_VERSION {
            return Err(Failure::argument("unsupported result schema version"));
        }
        let rule: MarkerRule = unsafe { parse_json(rule_json, rule_len) }?;
        let markers = analysis.scene_markers(&rule);
        unsafe { write_json(out, &markers) }
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_free(analyzer: *mut AnalyzerHandle) {
    if !analyzer.is_null() {
        // SAFETY: The caller transfers ownership of the original live handle.
        catch(|| unsafe { drop(Box::from_raw(analyzer)) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_buffer_free(buffer: *mut CBuffer) {
    // SAFETY: The caller supplies the original owned result or an empty buffer.
    if let Some(buffer) = unsafe { buffer.as_mut() } {
        let owned = std::mem::replace(buffer, CBuffer::EMPTY);
        if !owned.data.is_null() {
            let data = ptr::slice_from_raw_parts_mut(owned.data.cast_mut(), owned.len as usize);
            catch(|| unsafe { drop(Box::from_raw(data)) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_create_clears_output_and_reports_exact_resume() {
        let mut out = ptr::dangling_mut();
        let mut error = Failure::default().to_error();
        let start = CTime {
            pts: -27,
            numerator: 1,
            denominator: 0,
        };
        assert_eq!(
            unsafe { tiletrail_analyzer_create(ptr::null(), start, &mut out, &mut error) },
            2,
        );
        assert!(out.is_null());
        assert_eq!(error.code, 2);
        assert_eq!(error.has_resume, 1);
        assert_eq!(Timestamp::from(error.resume_at), Timestamp::from(start));
        assert_ne!(error.message[0], 0);
        assert_eq!(error.message[255], 0);
    }

    #[test]
    fn feed_constructor_selects_automatic_or_grid_and_clears_invalid_output() {
        let config = CConfig {
            mode: 1,
            sampling: 0,
        };
        let start = CTime::from(Timestamp::new(0, 1, 30));
        let mut error = Failure::default().to_error();
        for (columns, rows, expected) in [
            (0, 0, Some(FeedLayout::Automatic)),
            (
                3,
                2,
                Some(FeedLayout::Grid {
                    columns: 3,
                    rows: 2,
                }),
            ),
            (0, 2, None),
            (u32::MAX, u32::MAX, None),
        ] {
            let mut handle = ptr::dangling_mut();
            let status = unsafe {
                tiletrail_analyzer_create_feeds(
                    &config,
                    start,
                    columns,
                    rows,
                    &mut handle,
                    &mut error,
                )
            };
            if let Some(layout) = expected {
                assert_eq!(status, 0);
                assert_eq!(unsafe { &*handle }.analyzer.layout(), layout);
                unsafe { tiletrail_analyzer_free(handle) };
            } else {
                assert_eq!(status, 3);
                assert!(handle.is_null());
            }
        }
    }

    #[test]
    fn snapshots_survive_replay_and_analyzer_release() {
        let config = CConfig {
            mode: 1,
            sampling: 0,
        };
        let start = CTime::from(Timestamp::new(-5, 1001, 30_000));
        let end = CTime::from(Timestamp::new(-4, 1001, 30_000));
        let mut analyzer = ptr::null_mut();
        let mut error = Failure::default().to_error();
        let mut partial = CBuffer::EMPTY;
        let mut complete = CBuffer::EMPTY;
        let data = Box::new([127]);
        let mut frame = CFrame {
            data: data.as_ptr(),
            len: 0,
            stride: 1,
            width: 1,
            height: 1,
            format: 0,
            rotation: 0,
            pixel_aspect_num: 1,
            pixel_aspect_den: 1,
            timestamp: start,
        };
        unsafe {
            assert_eq!(
                tiletrail_analyzer_create(&config, start, &mut analyzer, &mut error),
                0
            );
            assert_eq!(tiletrail_analyzer_push(analyzer, &frame, &mut error), 1);
            assert_eq!(error.has_resume, 1);
            assert_eq!(Timestamp::from(error.resume_at), Timestamp::from(start));
            assert_eq!(
                tiletrail_analyzer_result_json(analyzer, end, &mut partial, &mut error),
                0
            );
            assert_eq!(error.code, 0);
            assert_eq!(error.has_resume, 0);
            assert_eq!(error.message[0], 0);
            frame.len = 1;
            assert_eq!(tiletrail_analyzer_push(analyzer, &frame, &mut error), 0);
            drop(data);
            assert_eq!(
                tiletrail_analyzer_result_json(analyzer, end, &mut complete, &mut error),
                0
            );
            tiletrail_analyzer_free(analyzer);

            let first: AnalysisResult = parse_json(partial.data, partial.len).unwrap();
            let second: AnalysisResult = parse_json(complete.data, complete.len).unwrap();
            assert_eq!(first.resume_at, Some(Timestamp::from(start)));
            assert!(first.observations.is_empty());
            assert_eq!(second.resume_at, None);
            assert_eq!(second.observations.len(), 1);

            let mut markers = CBuffer::EMPTY;
            let invalid_rule = b"{}";
            assert_eq!(
                tiletrail_scene_markers_json(
                    complete.data,
                    complete.len,
                    invalid_rule.as_ptr(),
                    invalid_rule.len() as u64,
                    &mut markers,
                    &mut error,
                ),
                6,
            );
            assert!(markers.data.is_null());
            assert_eq!(markers.len, 0);
            tiletrail_buffer_free(&mut partial);
            tiletrail_buffer_free(&mut complete);
            tiletrail_buffer_free(&mut complete);
            assert!(complete.data.is_null());
            assert_eq!(complete.len, 0);
        }
    }
}
