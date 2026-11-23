//! Analyze participant tiles in timestamped, caller-decoded video frames.
//!
//! Feed packed frames in increasing presentation order. Each call borrows its
//! pixels only for the duration of that call. The analyzer keeps measurements
//! and results, so the caller can reuse its decode buffer immediately.
//!
//! Construct an analyzer at the requested scan start, push frames, then call
//! [Analyzer::finish] with the exclusive end on the same presentation clock.
//! Use [Sampling::EveryFrame] only when submitting every decoded frame and
//! [Sampling::Sparse] when submitting a subset. Sparse analysis exposes sampled
//! observations and inferred intervals separately.

#![doc = include_str!("../README.md")]

mod ffi;
mod gpu;
mod markers;
mod model;
mod tracker;
pub(crate) mod vision;

pub use markers::*;
pub use model::*;

use gpu::BackendSession;
use tracker::Tracker;

const MIN_LAYOUT_CONFIDENCE: f32 = 0.4;

/// Stateful offline analyzer for one source stream.
pub struct Analyzer {
    config: AnalyzerConfig,
    start: Timestamp,
    tracker: Tracker,
    pub(crate) session: BackendSession,
    uncommitted: Option<Timestamp>,
}

impl Analyzer {
    /// Start the requested backend at the beginning of the scan.
    pub fn new(config: AnalyzerConfig, start: Timestamp) -> Result<Self, AnalysisError> {
        if !start.is_valid() {
            return Err(error(
                ErrorKind::InvalidTimestamp,
                "invalid scan start time base",
                start,
            ));
        }
        let session = BackendSession::new(config.mode, start)?;
        Ok(Self {
            config,
            start,
            tracker: Tracker::default(),
            session,
            uncommitted: None,
        })
    }

    /// Return the backend currently responsible for frame measurements.
    pub fn backend(&self) -> Backend {
        self.session.backend()
    }

    /// Return startup and backend transition diagnostics in order.
    pub fn diagnostics(&self) -> &[BackendDiagnostic] {
        self.session.diagnostics()
    }

    /// Measure and commit a single frame. Frames must arrive in increasing
    /// presentation order. An error leaves the last committed state intact.
    pub fn push(&mut self, frame: Frame<'_>) -> Result<(), AnalysisError> {
        let timestamp = frame.timestamp;
        if !timestamp.is_valid()
            || timestamp.before(self.start)
            || self
                .tracker
                .observations
                .last()
                .is_some_and(|last| !last.timestamp.before(timestamp))
        {
            return Err(error(
                ErrorKind::InvalidTimestamp,
                "frame timestamp must be at or after the scan start and strictly increasing",
                self.uncommitted.unwrap_or(timestamp),
            ));
        }
        if let Some(uncommitted) = self
            .uncommitted
            .filter(|&point| timestamp.compare(point) != Some(std::cmp::Ordering::Equal))
        {
            return Err(error(
                ErrorKind::InvalidTimestamp,
                "replay the uncommitted frame before continuing",
                uncommitted,
            ));
        }
        let measurements = validate_frame(&frame)
            .and_then(|()| self.session.measure(&frame))
            .map_err(|mut error| {
                // An equivalent clock can replay the frame. Keep the original
                // source ticks as the resume point if that attempt also fails.
                error.resume_at = *self.uncommitted.get_or_insert(timestamp);
                error
            })?;
        let detection = vision::detect(&measurements);
        self.tracker
            .push(&frame, detection, self.config.sampling == Sampling::Sparse);
        self.uncommitted = None;
        Ok(())
    }

    /// Produce a versioned snapshot ending at the exclusive scan end. Following
    /// a failed submission, the result reports incomplete coverage from the
    /// first uncommitted frame. The end may equal that timestamp, in which case
    /// the snapshot retains the resume point without an incomplete tail.
    /// The analyzer may continue accepting input.
    pub fn finish(&self, end: Timestamp) -> Result<AnalysisResult, AnalysisError> {
        self.snapshot(end).map(AnalysisSnapshot::into_owned)
    }

    /// Borrow a snapshot with the same contents and range checks as
    /// [`Analyzer::finish`]. Serialize it directly to a writer to avoid
    /// copying the frame history or building a JSON buffer in memory.
    ///
    /// ```
    /// # use tiletrail::{Analyzer, Timestamp};
    /// # fn save(analyzer: &Analyzer, end: Timestamp, writer: impl std::io::Write)
    /// #     -> Result<(), Box<dyn std::error::Error>> {
    /// serde_json::to_writer(writer, &analyzer.snapshot(end)?)?;
    /// # Ok(()) }
    /// ```
    pub fn snapshot(&self, end: Timestamp) -> Result<AnalysisSnapshot<'_>, AnalysisError> {
        if !self.start.before(end)
            || self
                .tracker
                .observations
                .last()
                .is_some_and(|last| !last.timestamp.before(end))
            || self.uncommitted.is_some_and(|point| end.before(point))
        {
            return Err(error(
                ErrorKind::InvalidRange,
                "scan end must be after the scan start and the last submitted frame",
                self.uncommitted.unwrap_or(end),
            ));
        }
        let covered_end = self.uncommitted.unwrap_or(end);
        let mut coverage = Vec::new();
        let observations = &self.tracker.observations;
        if let Some(first) = observations.first() {
            append_coverage(
                &mut coverage,
                self.start,
                first.timestamp,
                Coverage::Incomplete,
            );
        } else {
            append_coverage(&mut coverage, self.start, covered_end, Coverage::Incomplete);
        }
        for (index, observation) in observations.iter().enumerate() {
            let interval_end = observations
                .get(index + 1)
                .map_or(covered_end, |next| next.timestamp);
            let status = if observation.confidence < MIN_LAYOUT_CONFIDENCE {
                Coverage::Unknown
            } else if self.config.sampling == Sampling::Sparse {
                Coverage::Inferred
            } else {
                Coverage::Observed
            };
            append_coverage(&mut coverage, observation.timestamp, interval_end, status);
        }
        append_coverage(&mut coverage, covered_end, end, Coverage::Incomplete);

        Ok(AnalysisSnapshot {
            schema_version: RESULT_SCHEMA_VERSION,
            start: self.start,
            end,
            sampling: self.config.sampling,
            observations,
            tracks: &self.tracker.tracks,
            appearances: self.tracker.appearances_at_end(covered_end),
            events: &self.tracker.events,
            coverage,
            diagnostics: self.diagnostics(),
            resume_at: self.uncommitted,
        })
    }
}

fn validate_frame(frame: &Frame<'_>) -> Result<(), AnalysisError> {
    let fail = |message| error(ErrorKind::InvalidFrame, message, frame.timestamp);
    if frame.width == 0 || frame.height == 0 {
        return Err(fail("frame dimensions must be positive"));
    }
    if frame.display.pixel_aspect_num == 0 || frame.display.pixel_aspect_den == 0 {
        return Err(fail("pixel aspect ratio must be positive"));
    }
    let row_bytes = (frame.width as usize)
        .checked_mul(frame.format.bytes_per_pixel())
        .ok_or_else(|| fail("frame row size overflows"))?;
    if frame.stride < row_bytes {
        return Err(fail("frame stride is smaller than its visible row"));
    }
    let needed = (frame.height as usize - 1)
        .checked_mul(frame.stride)
        .and_then(|preceding_rows| preceding_rows.checked_add(row_bytes))
        .ok_or_else(|| fail("frame buffer extent overflows"))?;
    if frame.data.len() < needed {
        return Err(fail("frame buffer ends before the last visible row"));
    }
    Ok(())
}

fn error(kind: ErrorKind, message: &str, resume_at: Timestamp) -> AnalysisError {
    AnalysisError {
        kind,
        message: message.to_owned(),
        resume_at,
    }
}

fn append_coverage(
    intervals: &mut Vec<CoverageInterval>,
    start: Timestamp,
    end: Timestamp,
    status: Coverage,
) {
    if !start.before(end) {
        return;
    }
    if let Some(previous) = intervals.last_mut().filter(|interval| {
        interval.status == status && interval.end.compare(start) == Some(std::cmp::Ordering::Equal)
    }) {
        previous.end = end;
    } else {
        intervals.push(CoverageInterval { start, end, status });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u32 = 100;
    const HEIGHT: u32 = 56;
    const STRIDE: usize = WIDTH as usize * 3 + 7;

    fn time(pts: i64) -> Timestamp {
        Timestamp::new(pts, 1, 90_000)
    }

    fn gallery(swapped: bool) -> Vec<u8> {
        const ROW_BYTES: usize = WIDTH as usize * 3;
        let mut data = vec![0; (HEIGHT as usize - 1) * STRIDE + ROW_BYTES];
        for row in 0..HEIGHT as usize {
            for col in 0..WIDTH as usize {
                let pixel = if (47..53).contains(&col) {
                    [0, 0, 0]
                } else if (col < 47) ^ swapped {
                    [220, 24, 36]
                } else {
                    [26, 210, 44]
                };
                let index = row * STRIDE + col * 3;
                data[index..index + 3].copy_from_slice(&pixel);
            }
        }
        data
    }

    fn frame(data: &[u8], pts: i64) -> Frame<'_> {
        Frame {
            data,
            width: WIDTH,
            height: HEIGHT,
            stride: STRIDE,
            format: PixelFormat::Rgb24,
            timestamp: time(pts),
            display: DisplayGeometry::default(),
        }
    }

    fn analyzer(sampling: Sampling) -> Analyzer {
        Analyzer::new(
            AnalyzerConfig {
                mode: BackendMode::CpuOnly,
                sampling,
            },
            time(-3000),
        )
        .unwrap()
    }

    #[test]
    fn validates_borrowed_extent_and_exact_source_clock() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        let data = gallery(false);
        let bytes = &data[..data.len() - 1];
        let invalid = analyzer.push(frame(bytes, -3000)).unwrap_err();
        assert_eq!(invalid.kind, ErrorKind::InvalidFrame);
        assert_eq!(invalid.resume_at, time(-3000));
        let truncated = analyzer.finish(time(0)).unwrap();
        assert_eq!(
            truncated.coverage_at(time(-3000)),
            Some(Coverage::Incomplete)
        );
        assert_eq!(truncated.resume_at, Some(time(-3000)));
        assert!(analyzer.push(frame(&data, 0)).is_err());
        analyzer.push(frame(&data, -3000)).unwrap();
        let repeated = Frame {
            timestamp: Timestamp::new(-1, 1, 30),
            ..frame(&data, 0)
        };
        assert_eq!(
            analyzer.push(repeated).unwrap_err().kind,
            ErrorKind::InvalidTimestamp
        );
        analyzer.push(frame(&data, 0)).unwrap();
        let result = analyzer.finish(time(3000)).unwrap();
        assert_eq!(result.observations.len(), 2);
        assert_eq!(result.coverage_at(time(2999)), Some(Coverage::Observed));
        assert!(result.visible_at(time(3000)).is_none());
        assert_eq!(result.resume_at, None);
    }

    #[test]
    fn marks_unreadable_and_sparse_intervals() {
        let mut analyzer = analyzer(Sampling::Sparse);
        let data = gallery(false);
        let black = vec![0; data.len()];
        analyzer.push(frame(&data, 0)).unwrap();
        analyzer.push(frame(&black, 3000)).unwrap();
        analyzer.push(frame(&data, 6000)).unwrap();
        let result = analyzer.finish(time(9000)).unwrap();
        assert_eq!(result.coverage_at(time(-1)), Some(Coverage::Incomplete));
        assert_eq!(result.coverage_at(time(1000)), Some(Coverage::Inferred));
        assert_eq!(result.coverage_at(time(3000)), Some(Coverage::Unknown));
        assert_eq!(result.coverage_at(time(8999)), Some(Coverage::Inferred));
        assert!(result.visible_at(time(3500)).unwrap().regions.is_empty());
        assert_eq!(result.tracks.len(), 2, "{:?}", result.observations);
        assert_eq!(result.appearances_of(1, time(0), time(9000)).count(), 2);
        assert_eq!(result.appearances_of(1, time(3000), time(6000)).count(), 0);
        assert!(result.events.iter().all(|event| event.uncertain));
    }

    #[test]
    fn device_loss_auto_replays_swap_without_duplicate_events() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        analyzer.session = BackendSession::fake_gpu(BackendMode::Auto, time(-3000));
        let before = gallery(false);
        let after = gallery(true);
        analyzer.push(frame(&before, -3000)).unwrap();
        analyzer.session.inject_device_loss();
        analyzer.push(frame(&after, 0)).unwrap();
        analyzer.push(frame(&after, 3000)).unwrap();
        let result = analyzer.finish(time(6000)).unwrap();
        assert_eq!(analyzer.backend(), Backend::Cpu);
        assert_eq!(result.diagnostics.len(), 2);
        assert_eq!(result.diagnostics[1].timestamp, time(0));
        assert_eq!(result.resume_at, None);
        assert_eq!(result.tracks.len(), 2, "{:?}", result.observations);
        let left_before = &result.visible_at(time(-1)).unwrap().regions[0];
        let right_after = &result.visible_at(time(0)).unwrap().regions[1];
        assert_eq!(left_before.participant_id, right_after.participant_id);
        assert_eq!(left_before.appearance_id, right_after.appearance_id);
        assert_eq!(result.events.len(), 2, "{:?}", result.events);
        assert!(result.events.iter().all(|event| {
            event.timestamp == time(0) && event.kind == ChangeKind::Move && !event.uncertain
        }));
    }

    #[test]
    fn device_loss_required_keeps_exact_resume_and_partial_result() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        analyzer.session = BackendSession::fake_gpu(BackendMode::GpuRequired, time(-3000));
        let before = gallery(false);
        let after = gallery(true);
        analyzer.push(frame(&before, -3000)).unwrap();
        analyzer.session.inject_device_loss();
        let failure = analyzer.push(frame(&after, 0)).unwrap_err();
        assert_eq!(failure.kind, ErrorKind::DeviceLost);
        assert_eq!(failure.resume_at, time(0));
        assert_eq!(
            analyzer.push(frame(&after, 3000)).unwrap_err().resume_at,
            time(0)
        );
        let result = analyzer.finish(time(6000)).unwrap();
        assert_eq!(result.resume_at, Some(time(0)));
        assert_eq!(result.observations.len(), 1);
        assert_eq!(result.coverage_at(time(-1)), Some(Coverage::Observed));
        assert_eq!(result.coverage_at(time(0)), Some(Coverage::Incomplete));
        assert!(result.visible_at(time(0)).is_none());
        assert_eq!(result.appearances[0].end, time(0));
        assert!(result.events.is_empty());
    }

    #[test]
    fn snapshot_at_uncommitted_boundary_round_trips_exact_times() {
        let start = Timestamp::new(i64::MIN, u32::MAX, u32::MAX);
        let failed = Timestamp::new(i64::MAX - 1, u32::MAX, u32::MAX);
        let end = Timestamp::new(i64::MAX - 1, 1, 1);
        let mut analyzer = Analyzer::new(
            AnalyzerConfig {
                mode: BackendMode::CpuOnly,
                ..Default::default()
            },
            start,
        )
        .unwrap();
        let data = gallery(false);
        analyzer
            .push(Frame {
                timestamp: start,
                ..frame(&data, 0)
            })
            .unwrap();
        analyzer
            .push(Frame {
                timestamp: failed,
                ..frame(&[], 0)
            })
            .unwrap_err();

        let retry = analyzer
            .push(Frame {
                timestamp: end,
                ..frame(&[], 0)
            })
            .unwrap_err();
        assert_eq!(retry.resume_at, failed);
        let snapshot = analyzer.finish(end).unwrap();
        let json = serde_json::to_vec(&analyzer.snapshot(end).unwrap()).unwrap();
        assert_eq!(json, serde_json::to_vec(&snapshot).unwrap());
        let restored: AnalysisResult = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored.start, start);
        assert_eq!(restored.end, end);
        assert_eq!(restored.resume_at, Some(failed));
        assert_eq!(restored.coverage.len(), 1);
        assert_eq!(restored.coverage_at(start), Some(Coverage::Observed));
        assert_eq!(restored.coverage_at(failed), None);

        analyzer
            .push(Frame {
                timestamp: end,
                ..frame(&data, 0)
            })
            .unwrap();
        let resumed = analyzer.finish(Timestamp::new(i64::MAX, 1, 1)).unwrap();
        assert_eq!(resumed.resume_at, None);
        assert_eq!(resumed.observations.len(), 2);
        assert_eq!(resumed.coverage_at(failed), Some(Coverage::Observed));
    }
}
