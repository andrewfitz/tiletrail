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

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_create_gallery(
    config: *const CConfig,
    start: CTime,
    columns: u32,
    rows: u32,
    out: *mut *mut AnalyzerHandle,
    error: *mut CError,
) -> i32 {
    create_analyzer(
        config,
        start,
        FeedLayout::Gallery { columns, rows },
        out,
        error,
    )
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

/// Borrow a caller frame for the duration of one submission.
///
/// # Safety
/// The frame must describe `len` readable bytes that outlive the result.
unsafe fn borrow_frame<'a>(frame: &CFrame) -> Result<Frame<'a>, Failure> {
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
    Ok(Frame {
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
        let frame = unsafe { borrow_frame(frame) }?;
        handle.panicked = true;
        let result = handle.analyzer.push(frame);
        handle.panicked = false;
        result.map_err(Failure::from)
    })
}

const COARSE_COMMITTED: u32 = 0;
const COARSE_REFINE: u32 = 1;

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_push_coarse(
    analyzer: *mut AnalyzerHandle,
    frame: *const CFrame,
    outcome: *mut u32,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if outcome.is_null() {
            return Err(Failure::argument("missing coarse outcome"));
        }
        // A failed call must never read as a committed frame.
        // SAFETY: The caller supplies writable, aligned outcome storage.
        unsafe { outcome.write(COARSE_REFINE) };
        // SAFETY: The caller guarantees serialized access and valid storage.
        let handle =
            unsafe { analyzer.as_mut() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let frame = unsafe { frame.as_ref() }.ok_or_else(|| Failure::argument("missing frame"))?;
        let frame = unsafe { borrow_frame(frame) }?;
        handle.panicked = true;
        let result = handle.analyzer.push_coarse(frame);
        handle.panicked = false;
        let committed = result? == crate::CoarseOutcome::Committed;
        unsafe {
            outcome.write(if committed {
                COARSE_COMMITTED
            } else {
                COARSE_REFINE
            })
        };
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_prepare(
    analyzer: *const AnalyzerHandle,
    frame: *const CFrame,
    out: *mut *mut crate::PreparedFrame,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if out.is_null() {
            return Err(Failure::argument("missing prepared output"));
        }
        // SAFETY: The caller supplies writable output storage.
        unsafe { out.write(ptr::null_mut()) };
        // SAFETY: No push runs during preparation, so shared access is sound.
        let handle =
            unsafe { analyzer.as_ref() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let frame = unsafe { frame.as_ref() }.ok_or_else(|| Failure::argument("missing frame"))?;
        let frame = unsafe { borrow_frame(frame) }?;
        let prepared = handle.analyzer.prepare(&frame)?;
        unsafe { out.write(Box::into_raw(Box::new(prepared))) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_push_prepared(
    analyzer: *mut AnalyzerHandle,
    prepared: *mut crate::PreparedFrame,
    coarse: u32,
    outcome: *mut u32,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if !outcome.is_null() {
            // A failed call must never read as a committed frame.
            // SAFETY: The caller supplies writable, aligned outcome storage.
            unsafe { outcome.write(COARSE_REFINE) };
        }
        if prepared.is_null() {
            return Err(Failure::argument("missing prepared frame"));
        }
        // SAFETY: The caller transfers ownership of a live prepared frame.
        let prepared = unsafe { Box::from_raw(prepared) };
        if coarse != 0 && outcome.is_null() {
            return Err(Failure::argument("missing coarse outcome"));
        }
        // SAFETY: The caller guarantees serialized access and valid storage.
        let handle =
            unsafe { analyzer.as_mut() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        handle.panicked = true;
        let result = if coarse != 0 {
            handle.analyzer.push_coarse_prepared(*prepared)
        } else {
            handle
                .analyzer
                .push_prepared(*prepared)
                .map(|()| crate::CoarseOutcome::Committed)
        };
        handle.panicked = false;
        let committed = result? == crate::CoarseOutcome::Committed;
        if !outcome.is_null() {
            unsafe {
                outcome.write(if committed {
                    COARSE_COMMITTED
                } else {
                    COARSE_REFINE
                })
            };
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_prepared_free(prepared: *mut crate::PreparedFrame) {
    if !prepared.is_null() {
        // SAFETY: The caller transfers ownership of a live prepared frame.
        catch(|| unsafe { drop(Box::from_raw(prepared)) });
    }
}

#[repr(C)]
#[derive(Default)]
struct CProgress {
    frames_processed: u64,
    last_timestamp: CTime,
    state: u32,
    percent: i32,
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_progress(
    analyzer: *const AnalyzerHandle,
    expected_end: *const CTime,
    out: *mut CProgress,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        if out.is_null() {
            return Err(Failure::argument("missing progress output"));
        }
        // SAFETY: the caller supplies writable output and valid, aligned inputs.
        unsafe {
            out.write(CProgress {
                percent: -1,
                ..CProgress::default()
            })
        };
        let handle =
            unsafe { analyzer.as_ref() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let end = unsafe { expected_end.as_ref() }
            .copied()
            .map(Timestamp::from);
        let progress = handle.analyzer.progress(end)?;
        let state = match progress.state {
            crate::ProgressState::Ready => 0,
            crate::ProgressState::Analyzing => 1,
            crate::ProgressState::NeedsReplay => 2,
            crate::ProgressState::Complete => 3,
        };
        unsafe {
            out.write(CProgress {
                frames_processed: progress.frames_processed as u64,
                last_timestamp: progress.last_timestamp.map(CTime::from).unwrap_or_default(),
                state,
                percent: progress.percent.map(i32::from).unwrap_or(-1),
            })
        };
        Ok(())
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

type CWrite = unsafe extern "C" fn(*mut std::ffi::c_void, *const u8, usize) -> i32;
type CRead = unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, usize) -> isize;
type CResultChunk = unsafe extern "C" fn(*mut std::ffi::c_void, u32, *const u8, usize) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CCursor {
    observations: usize,
    events: usize,
    coarse: usize,
}

impl From<crate::journal::Cursor> for CCursor {
    fn from(cursor: crate::journal::Cursor) -> Self {
        Self {
            observations: cursor.observations,
            events: cursor.events,
            coarse: cursor.coarse,
        }
    }
}

fn io_failure(error: impl std::fmt::Display) -> Failure {
    Failure {
        code: 8,
        message: error.to_string(),
        ..Failure::default()
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_checkpoint_write(
    analyzer: *const AnalyzerHandle,
    cursor: *mut CCursor,
    write: Option<CWrite>,
    context: *mut std::ffi::c_void,
    error: *mut CError,
) -> i32 {
    use std::io::{self, Write};
    struct Writer(CWrite, *mut std::ffi::c_void);
    impl Write for Writer {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if unsafe { (self.0)(self.1, data.as_ptr(), data.len()) } != 0 {
                return Err(io::Error::other("checkpoint write failed"));
            }
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    boundary(error, || {
        let handle =
            unsafe { analyzer.as_ref() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        let cursor = unsafe { cursor.as_mut() }
            .ok_or_else(|| Failure::argument("missing checkpoint cursor"))?;
        let write = write.ok_or_else(|| Failure::argument("missing checkpoint writer"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let previous = crate::journal::Cursor {
            observations: cursor.observations,
            events: cursor.events,
            coarse: cursor.coarse,
        };
        let mut writer = io::BufWriter::with_capacity(64 * 1024, Writer(write, context));
        handle
            .analyzer
            .write_checkpoint(previous, &mut writer)
            .map_err(io_failure)?;
        writer.flush().map_err(io_failure)?;
        *cursor = handle.analyzer.journal_cursor().into();
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_checkpoint_read(
    analyzer: *mut AnalyzerHandle,
    cursor: *mut CCursor,
    read: Option<CRead>,
    context: *mut std::ffi::c_void,
    error: *mut CError,
) -> i32 {
    use std::io::{self, Read};
    struct Reader(CRead, *mut std::ffi::c_void);
    impl Read for Reader {
        fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
            let count = unsafe { (self.0)(self.1, data.as_mut_ptr(), data.len()) };
            usize::try_from(count)
                .ok()
                .filter(|&count| count <= data.len())
                .ok_or_else(|| io::Error::other("checkpoint read failed"))
        }
    }
    boundary(error, || {
        let handle =
            unsafe { analyzer.as_mut() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        let cursor = unsafe { cursor.as_mut() }
            .ok_or_else(|| Failure::argument("missing checkpoint cursor"))?;
        let read = read.ok_or_else(|| Failure::argument("missing checkpoint reader"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let reader = io::BufReader::with_capacity(64 * 1024, Reader(read, context));
        handle
            .analyzer
            .read_checkpoint(reader)
            .map_err(io_failure)?;
        *cursor = handle.analyzer.journal_cursor().into();
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tiletrail_analyzer_result_chunks(
    analyzer: *const AnalyzerHandle,
    end: CTime,
    chunk: Option<CResultChunk>,
    context: *mut std::ffi::c_void,
    error: *mut CError,
) -> i32 {
    boundary(error, || {
        let handle =
            unsafe { analyzer.as_ref() }.ok_or_else(|| Failure::argument("missing analyzer"))?;
        let chunk = chunk.ok_or_else(|| Failure::argument("missing result callback"))?;
        if handle.panicked {
            return Err(Failure::panic());
        }
        let result = handle.analyzer.snapshot(end.into())?;
        let header = crate::AnalysisSnapshot {
            observations: &[],
            tracks: &[],
            appearances: Vec::new(),
            events: &[],
            coverage: Vec::new(),
            diagnostics: &[],
            ..result
        };
        fn emit(
            callback: CResultChunk,
            context: *mut std::ffi::c_void,
            kind: u32,
            data: &impl serde::Serialize,
            buffer: &mut Vec<u8>,
        ) -> Result<(), Failure> {
            buffer.clear();
            serde_json::to_writer(&mut *buffer, data).map_err(io_failure)?;
            if unsafe { callback(context, kind, buffer.as_ptr(), buffer.len()) } != 0 {
                return Err(io_failure("result callback stopped"));
            }
            Ok(())
        }
        let mut buffer = Vec::new();
        emit(chunk, context, 0, &header, &mut buffer)?;
        macro_rules! rows {
            ($kind:literal, $field:ident) => {
                for batch in result.$field.chunks(32) {
                    emit(chunk, context, $kind, &batch, &mut buffer)?;
                }
            };
        }
        rows!(1, observations);
        rows!(2, tracks);
        rows!(3, appearances);
        rows!(4, events);
        rows!(5, coverage);
        rows!(6, diagnostics);
        Ok(())
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
    fn c_progress_reports_pending_range_and_clears_output_on_error() {
        let config = CConfig {
            mode: 1,
            sampling: 0,
        };
        let start = CTime::from(Timestamp::new(0, 1, 1));
        let end = CTime::from(Timestamp::new(2, 1, 1));
        let mut handle = ptr::null_mut();
        let mut error = Failure::default().to_error();
        let mut progress = CProgress::default();
        let data = [1];
        let mut frame = CFrame {
            data: data.as_ptr(),
            len: 1,
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
                tiletrail_analyzer_create(&config, start, &mut handle, &mut error),
                0
            );
            assert_eq!(
                tiletrail_analyzer_progress(handle, &end, &mut progress, &mut error),
                0
            );
            assert_eq!(
                (progress.frames_processed, progress.state, progress.percent),
                (0, 0, 0)
            );
            assert_eq!(tiletrail_analyzer_push(handle, &frame, &mut error), 0);
            frame.timestamp = CTime::from(Timestamp::new(3, 1, 1));
            frame.len = 0;
            assert_eq!(tiletrail_analyzer_push(handle, &frame, &mut error), 1);
            assert_eq!(
                tiletrail_analyzer_progress(handle, &frame.timestamp, &mut progress, &mut error),
                0
            );
            assert_eq!(
                (progress.frames_processed, progress.state, progress.percent),
                (1, 2, 0)
            );
            assert_eq!(
                tiletrail_analyzer_progress(handle, &end, &mut progress, &mut error),
                3
            );
            assert_eq!((error.has_resume, error.resume_at.pts), (1, 3));
            assert_eq!((progress.frames_processed, progress.percent), (0, -1));
            assert_eq!(
                tiletrail_analyzer_progress(handle, ptr::null(), &mut progress, &mut error),
                0
            );
            assert_eq!(
                (progress.frames_processed, progress.state, progress.percent),
                (1, 2, -1)
            );
            tiletrail_analyzer_free(handle);
        }
    }

    #[test]
    fn coarse_push_reports_its_outcome_and_never_commits_on_failure() {
        let config = CConfig {
            mode: 1,
            sampling: 0,
        };
        let start = CTime::from(Timestamp::new(0, 1, 30));
        let mut analyzer = ptr::null_mut();
        let mut error = Failure::default().to_error();
        let data = [127u8];
        let mut frame = CFrame {
            data: data.as_ptr(),
            len: 1,
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
            assert_eq!(
                tiletrail_analyzer_push_coarse(analyzer, &frame, ptr::null_mut(), &mut error),
                6
            );
            // The first frame follows nothing and is committed.
            let mut outcome = u32::MAX;
            assert_eq!(
                tiletrail_analyzer_push_coarse(analyzer, &frame, &mut outcome, &mut error),
                0
            );
            assert_eq!(outcome, COARSE_COMMITTED);
            // A rejected frame reads as a refusal.
            frame.timestamp = CTime::from(Timestamp::new(90, 1, 30));
            frame.len = 0;
            assert_eq!(
                tiletrail_analyzer_push_coarse(analyzer, &frame, &mut outcome, &mut error),
                1
            );
            assert_eq!(outcome, COARSE_REFINE);
            // A prepared frame commits like the frame and is consumed either way.
            frame.len = 1;
            let mut prepared = ptr::null_mut();
            assert_eq!(
                tiletrail_analyzer_prepare(analyzer, &frame, &mut prepared, &mut error),
                0
            );
            assert_eq!(
                tiletrail_analyzer_push_prepared(analyzer, prepared, 1, &mut outcome, &mut error),
                0
            );
            assert_eq!(outcome, COARSE_COMMITTED);
            assert_eq!(
                tiletrail_analyzer_prepare(analyzer, &frame, &mut prepared, &mut error),
                0
            );
            assert_eq!(
                tiletrail_analyzer_push_prepared(
                    analyzer,
                    prepared,
                    0,
                    ptr::null_mut(),
                    &mut error
                ),
                2
            );
            assert_eq!(
                tiletrail_analyzer_prepare(analyzer, &frame, &mut prepared, &mut error),
                0
            );
            tiletrail_prepared_free(prepared);
            tiletrail_prepared_free(ptr::null_mut());
            let mut progress = CProgress::default();
            let end = CTime::from(Timestamp::new(120, 1, 30));
            assert_eq!(
                tiletrail_analyzer_progress(analyzer, &end, &mut progress, &mut error),
                0
            );
            assert_eq!(progress.frames_processed, 2);
            tiletrail_analyzer_free(analyzer);
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
            let mut progress = CProgress::default();
            assert_eq!(
                tiletrail_analyzer_progress(analyzer, &end, &mut progress, &mut error),
                0
            );
            assert_eq!(
                (progress.frames_processed, progress.state, progress.percent),
                (1, 1, 0)
            );
            assert_eq!(
                tiletrail_analyzer_progress(analyzer, ptr::null(), &mut progress, &mut error),
                0
            );
            assert_eq!(progress.percent, -1);
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
