//! Analyze feed regions in timestamped, caller-decoded video frames.
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
mod progress;
mod tracker;
pub(crate) mod vision;

pub use markers::*;
pub use model::*;
pub use progress::{AnalysisProgress, ProgressState};

use gpu::BackendSession;
use tracker::Tracker;

const MIN_LAYOUT_CONFIDENCE: f32 = 0.4;

/// Stateful offline analyzer for one source stream.
pub struct Analyzer {
    config: AnalyzerConfig,
    layout: FeedLayout,
    start: Timestamp,
    tracker: Tracker,
    pub(crate) session: BackendSession,
    uncommitted: Option<Timestamp>,
    /// Indices of observations committed by a coarse push, in order. The
    /// caller skipped the frames between each one and its predecessor.
    coarse: Vec<usize>,
    /// The uncommitted frame was a coarse submission, so the frames before it
    /// were skipped and never confirmed.
    coarse_pending: bool,
}

/// A frame measured and detected ahead of its push. See [`Analyzer::prepare`].
pub struct PreparedFrame {
    timestamp: Timestamp,
    display: DisplayGeometry,
    measurements: vision::Measurements,
    detection: vision::Detection,
}

/// Result of [`Analyzer::push_coarse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoarseOutcome {
    /// Nothing tracked changed since the last committed frame. The frame is
    /// committed and the skipped interval is reported as inferred.
    Committed,
    /// The frame differs from the last committed frame. The analyzer is
    /// unchanged. Push the skipped frames, then this frame, with
    /// [`Analyzer::push`].
    Refine,
}

impl Analyzer {
    /// Start the requested backend at the beginning of the scan.
    pub fn new(config: AnalyzerConfig, start: Timestamp) -> Result<Self, AnalysisError> {
        Self::with_layout(config, start, FeedLayout::Meeting)
    }

    /// Start an analyzer with an explicit feed layout.
    pub fn with_layout(
        config: AnalyzerConfig,
        start: Timestamp,
        layout: FeedLayout,
    ) -> Result<Self, AnalysisError> {
        if !start.is_valid() {
            return Err(error(
                ErrorKind::InvalidTimestamp,
                "invalid scan start time base",
                start,
            ));
        }
        if let FeedLayout::Grid { columns, rows } = layout {
            if !matches!(columns.checked_mul(rows), Some(1..=MAX_GRID_FEEDS)) {
                return Err(error(
                    ErrorKind::InvalidRange,
                    "grid dimensions must describe between 1 and 64 feeds",
                    start,
                ));
            }
        }
        let session = BackendSession::new(config.mode, start)?;
        Ok(Self {
            config,
            layout,
            start,
            tracker: Tracker::default(),
            session,
            uncommitted: None,
            coarse: Vec::new(),
            coarse_pending: false,
        })
    }

    /// Return the configured feed layout.
    pub fn layout(&self) -> FeedLayout {
        self.layout
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
        self.submit(frame, false).map(|_| ())
    }

    /// Submit a frame that follows frames the caller skipped, such as the
    /// next keyframe of a long recording. The frame is committed only when
    /// the tracker sees every region continue from the last committed frame:
    /// the same appearances and identity state, no event, and geometry within
    /// the tolerance of a move or resize. The skipped interval is then
    /// reported as [`Coverage::Inferred`].
    ///
    /// Otherwise the analyzer is left exactly as it was and the call returns
    /// [`CoarseOutcome::Refine`]. Push the skipped frames and then this frame
    /// with [`Analyzer::push`]; the result equals one that never tried the
    /// coarse step. A change that appears and fully reverts between two
    /// committed frames is not observed, which the inferred coverage records.
    ///
    /// The first frame of a scan has no skipped predecessor and is committed
    /// as a normal push. Timestamp rules and errors match [`Analyzer::push`].
    pub fn push_coarse(&mut self, frame: Frame<'_>) -> Result<CoarseOutcome, AnalysisError> {
        self.submit(frame, true)
    }

    /// Measure and detect a frame without changing the analyzer, so that
    /// several frames can be prepared at once on different threads. Push the
    /// results in presentation order with [`Analyzer::push_prepared`] or
    /// [`Analyzer::push_coarse_prepared`]; the outcome equals pushing the
    /// frames themselves. Preparation always measures on the CPU and fails
    /// for an analyzer that measures on the GPU. Do not prepare while a push
    /// is running on another thread.
    pub fn prepare(&self, frame: &Frame<'_>) -> Result<PreparedFrame, AnalysisError> {
        if self.session.backend() != Backend::Cpu {
            return Err(error(
                ErrorKind::InvalidFrame,
                "frames can be prepared only for CPU measurement",
                frame.timestamp,
            ));
        }
        validate_frame(frame)?;
        let measurements = vision::measure(frame);
        let detection = self.detect(&measurements);
        Ok(PreparedFrame {
            timestamp: frame.timestamp,
            display: frame.display,
            measurements,
            detection,
        })
    }

    /// Commit a prepared frame. Timestamp rules match [`Analyzer::push`].
    pub fn push_prepared(&mut self, prepared: PreparedFrame) -> Result<(), AnalysisError> {
        self.admit(prepared.timestamp)?;
        self.track(prepared, false);
        Ok(())
    }

    /// Offer a prepared frame that follows skipped frames. See
    /// [`Analyzer::push_coarse`].
    pub fn push_coarse_prepared(
        &mut self,
        prepared: PreparedFrame,
    ) -> Result<CoarseOutcome, AnalysisError> {
        self.admit(prepared.timestamp)?;
        Ok(self.track(prepared, true))
    }

    fn submit(&mut self, frame: Frame<'_>, coarse: bool) -> Result<CoarseOutcome, AnalysisError> {
        let timestamp = frame.timestamp;
        self.admit(timestamp)?;
        let coarse = coarse && !self.tracker.observations.is_empty();
        let measurements = validate_frame(&frame)
            .and_then(|()| self.session.measure(&frame))
            .map_err(|mut error| {
                // An equivalent clock can replay the frame. Keep the original
                // source ticks as the resume point if that attempt also fails.
                if self.uncommitted.is_none() {
                    self.coarse_pending = coarse;
                }
                error.resume_at = *self.uncommitted.get_or_insert(timestamp);
                error
            })?;
        let detection = self.detect(&measurements);
        let prepared = PreparedFrame {
            timestamp,
            display: frame.display,
            measurements,
            detection,
        };
        Ok(self.track(prepared, coarse))
    }

    /// Check that a frame may follow the committed history.
    fn admit(&self, timestamp: Timestamp) -> Result<(), AnalysisError> {
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
        Ok(())
    }

    /// The part of the analysis that reads only the frame.
    fn detect(&self, measurements: &vision::Measurements) -> vision::Detection {
        match self.layout {
            FeedLayout::Meeting => vision::detect(measurements),
            FeedLayout::Automatic => vision::detect_feeds(measurements),
            FeedLayout::Grid { columns, rows } => vision::grid_feeds(measurements, columns, rows),
        }
    }

    /// Associate a detected frame with the history and commit it, unless it
    /// was offered in place of skipped frames and something changed.
    fn track(&mut self, prepared: PreparedFrame, coarse: bool) -> CoarseOutcome {
        let coarse = coarse && !self.tracker.observations.is_empty();
        let PreparedFrame {
            timestamp,
            display,
            measurements,
            mut detection,
        } = prepared;
        // The tracker reads the timestamp and display geometry only.
        let frame = Frame {
            data: &[],
            width: measurements.width,
            height: measurements.height,
            stride: 0,
            format: PixelFormat::Gray8,
            timestamp,
            display,
        };
        let saved = coarse.then(|| self.tracker.checkpoint());
        let sparse = self.config.sampling == Sampling::Sparse;
        match self.layout {
            FeedLayout::Meeting => {
                self.tracker
                    .verify_thumbnail_subdivision(&measurements, &mut detection);
                self.tracker
                    .hold_established_tiles(&measurements, &mut detection, timestamp);
                self.tracker.push(&frame, detection, sparse);
            }
            FeedLayout::Automatic => self.tracker.push(&frame, detection, sparse),
            FeedLayout::Grid { .. } => self.tracker.push_grid(&frame, detection, sparse),
        }
        self.uncommitted = None;
        self.coarse_pending = false;
        if let Some(saved) = saved {
            if !self.tracker.continues(&saved) {
                self.tracker.restore(saved);
                return CoarseOutcome::Refine;
            }
            self.coarse.push(self.tracker.observations.len() - 1);
        }
        CoarseOutcome::Committed
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
        progress::validate_scan_range(
            self.start,
            self.tracker
                .observations
                .last()
                .map(|frame| frame.timestamp),
            self.uncommitted,
            end,
        )?;
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
        let mut coarse = self.coarse.iter().copied().peekable();
        for (index, observation) in observations.iter().enumerate() {
            let interval_end = observations
                .get(index + 1)
                .map_or(covered_end, |next| next.timestamp);
            while coarse.next_if(|&committed| committed <= index).is_some() {}
            let last = index + 1 == observations.len();
            let status = if last && self.coarse_pending {
                // A failed coarse frame confirmed nothing about the frames
                // skipped before it.
                Coverage::Incomplete
            } else if observation.confidence < MIN_LAYOUT_CONFIDENCE {
                Coverage::Unknown
            } else if self.config.sampling == Sampling::Sparse
                || coarse.peek() == Some(&(index + 1))
            {
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
    fn progress_tracks_committed_source_time_across_replay_and_snapshots() {
        let mut analyzer = analyzer(Sampling::Sparse);
        let end = Timestamp::new(1, 1, 30);
        let ready = analyzer.progress(Some(end)).unwrap();
        assert_eq!(
            (ready.state, ready.percent, ready.frames_processed),
            (ProgressState::Ready, Some(0), 0)
        );
        assert_eq!(analyzer.progress(None).unwrap().percent, None);
        let pixels = gallery(false);
        analyzer.push(frame(&pixels, -3000)).unwrap();
        analyzer.push(frame(&[], 0)).unwrap_err();
        let partial = analyzer.progress(Some(end)).unwrap();
        assert_eq!(partial.state, ProgressState::NeedsReplay);
        assert_eq!(partial.frames_processed, 1);
        assert_eq!(partial.last_timestamp, Some(time(-3000)));
        assert_eq!(analyzer.finish(end).unwrap().progress().unwrap(), partial);
        analyzer.push(frame(&pixels, 0)).unwrap();
        let middle = analyzer.progress(Some(end)).unwrap();
        assert_eq!(
            (middle.state, middle.percent, middle.frames_processed),
            (ProgressState::Analyzing, Some(50), 2)
        );
        assert!(analyzer.progress(Some(time(0))).is_err());
        let finished = analyzer.finish(end).unwrap().progress().unwrap();
        assert_eq!(
            (finished.state, finished.percent),
            (ProgressState::Complete, Some(100))
        );
        assert_eq!(analyzer.progress(Some(end)).unwrap(), middle);
    }

    #[test]
    fn progress_percentage_floors_exactly_on_mixed_source_clocks() {
        let mut a = Analyzer::new(
            AnalyzerConfig {
                mode: BackendMode::CpuOnly,
                ..Default::default()
            },
            Timestamp::new(0, 1, 30),
        )
        .unwrap();
        let pixels = gallery(false);
        for percent in 0..100 {
            let timestamp = Timestamp::new(percent, 3, 100);
            a.push(Frame {
                timestamp,
                ..frame(&pixels, 0)
            })
            .unwrap();
            assert_eq!(
                a.progress(Some(Timestamp::new(3, 1001, 1001)))
                    .unwrap()
                    .percent,
                Some(percent as u8),
                "timestamp {percent} / 100 of duration"
            );
        }

        let max = u32::MAX;
        let mut a = Analyzer::new(
            AnalyzerConfig {
                mode: BackendMode::CpuOnly,
                ..Default::default()
            },
            Timestamp::new(i64::MIN, max, max),
        )
        .unwrap();
        let end = Timestamp::new(i64::MAX - 1, max - 2, max - 2);
        for (pts, expected) in [(-2, 49), (-1, 50)] {
            a.push(Frame {
                timestamp: Timestamp::new(pts, max - 1, max - 1),
                ..frame(&pixels, 0)
            })
            .unwrap();
            assert_eq!(a.progress(Some(end)).unwrap().percent, Some(expected));
        }
    }

    #[test]
    fn live_progress_checks_resume_against_requested_end() {
        let mut a = analyzer(Sampling::EveryFrame);
        a.push(frame(&gallery(false), -3000)).unwrap();
        a.push(frame(&[], 3000)).unwrap_err();
        let error = a.progress(Some(time(1000))).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidRange);
        assert_eq!(error.resume_at, time(3000));
        let partial = a.progress(Some(time(3000))).unwrap();
        assert_eq!(partial.state, ProgressState::NeedsReplay);
        let mut result = a.finish(time(3000)).unwrap();
        assert_eq!(result.progress().unwrap(), partial);
        for invalid in [
            time(4000),
            time(-6000),
            Timestamp::new(0, 1, 0),
            time(-3000),
        ] {
            result.resume_at = Some(invalid);
            assert_eq!(result.progress().unwrap_err().kind, ErrorKind::InvalidRange);
        }
    }

    #[test]
    fn coarse_push_commits_only_a_continuing_layout_and_marks_the_skip_inferred() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        let data = gallery(false);
        // The first frame has no skipped predecessor.
        assert_eq!(
            analyzer.push_coarse(frame(&data, 0)).unwrap(),
            CoarseOutcome::Committed
        );
        analyzer.push(frame(&data, 3000)).unwrap();
        assert_eq!(
            analyzer.push_coarse(frame(&data, 219_000)).unwrap(),
            CoarseOutcome::Committed
        );
        analyzer.push(frame(&data, 222_000)).unwrap();
        let result = analyzer.finish(time(225_000)).unwrap();
        assert_eq!(result.observations.len(), 4);
        assert_eq!(result.sampling, Sampling::EveryFrame);
        assert_eq!(result.coverage_at(time(-1)), Some(Coverage::Incomplete));
        assert_eq!(result.coverage_at(time(0)), Some(Coverage::Observed));
        assert_eq!(result.coverage_at(time(3000)), Some(Coverage::Inferred));
        assert_eq!(result.coverage_at(time(218_999)), Some(Coverage::Inferred));
        assert_eq!(result.coverage_at(time(219_000)), Some(Coverage::Observed));
        assert_eq!(result.coverage_at(time(224_999)), Some(Coverage::Observed));
        assert!(result.events.is_empty());
        assert_eq!(result.appearances.len(), 2);
    }

    #[test]
    fn a_refused_coarse_push_leaves_no_trace_in_the_result() {
        let before = gallery(false);
        let after = gallery(true);
        let black = vec![0; before.len()];
        for changed in [&after, &black] {
            let mut dense = analyzer(Sampling::EveryFrame);
            let mut coarse = analyzer(Sampling::EveryFrame);
            for analyzer in [&mut dense, &mut coarse] {
                analyzer.push(frame(&before, 0)).unwrap();
                analyzer.push(frame(&before, 3000)).unwrap();
            }
            // Refused twice, including a retry at a different skipped distance.
            for pts in [9000, 12_000] {
                assert_eq!(
                    coarse.push_coarse(frame(changed, pts)).unwrap(),
                    CoarseOutcome::Refine
                );
            }
            for analyzer in [&mut dense, &mut coarse] {
                analyzer.push(frame(&before, 6000)).unwrap();
                analyzer.push(frame(changed, 9000)).unwrap();
                analyzer.push(frame(changed, 12_000)).unwrap();
            }
            let json = |analyzer: &Analyzer| {
                serde_json::to_vec(&analyzer.snapshot(time(15_000)).unwrap()).unwrap()
            };
            assert_eq!(json(&dense), json(&coarse));
        }
    }

    #[test]
    fn coarse_push_refuses_an_unreadable_layout_on_either_side() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        let data = gallery(false);
        let black = vec![0; data.len()];
        analyzer.push(frame(&black, 0)).unwrap();
        for pixels in [&black, &data] {
            assert_eq!(
                analyzer.push_coarse(frame(pixels, 3000)).unwrap(),
                CoarseOutcome::Refine
            );
        }
        assert_eq!(analyzer.finish(time(3000)).unwrap().observations.len(), 1);
    }

    #[test]
    fn a_failed_coarse_frame_leaves_the_skipped_interval_incomplete() {
        let mut analyzer = analyzer(Sampling::EveryFrame);
        let data = gallery(false);
        analyzer.push(frame(&data, 0)).unwrap();
        let failure = analyzer.push_coarse(frame(&[], 9000)).unwrap_err();
        assert_eq!(failure.resume_at, time(9000));
        let partial = analyzer.finish(time(12_000)).unwrap();
        assert_eq!(partial.coverage_at(time(3000)), Some(Coverage::Incomplete));
        assert_eq!(partial.resume_at, Some(time(9000)));
        assert_eq!(
            analyzer.push_coarse(frame(&data, 9000)).unwrap(),
            CoarseOutcome::Committed
        );
        let result = analyzer.finish(time(12_000)).unwrap();
        assert_eq!(result.coverage_at(time(3000)), Some(Coverage::Inferred));
        assert_eq!(result.coverage_at(time(9000)), Some(Coverage::Observed));
        assert_eq!(result.resume_at, None);
    }

    #[test]
    fn a_frame_sampled_by_the_published_rule_measures_like_its_source() {
        // TILETRAIL_SAMPLE_LIMIT in the C header documents this rule.
        const LIMIT: usize = 256;
        for (width, height) in [
            (1920usize, 1080usize),
            (1080, 1920),
            (641, 359),
            (300, 7),
            (256, 144),
        ] {
            let stride = width * 3 + 5;
            let data: Vec<u8> = (0..stride * height)
                .map(|index| ((index * 31 + index / 13) % 253) as u8)
                .collect();
            let longest = width.max(height);
            let scale = |size: usize| ((size * LIMIT + longest / 2) / longest).max(1);
            let (columns, rows) = if longest <= LIMIT {
                (width, height)
            } else {
                (scale(width), scale(height))
            };
            let mut sampled = Vec::with_capacity(columns * rows * 3);
            for y in 0..rows {
                let row = ((2 * y + 1) * height) / (2 * rows);
                for x in 0..columns {
                    let column = ((2 * x + 1) * width) / (2 * columns);
                    let at = row * stride + column * 3;
                    sampled.extend_from_slice(&data[at..at + 3]);
                }
            }
            let source = Frame {
                data: &data,
                width: width as u32,
                height: height as u32,
                stride,
                format: PixelFormat::Rgb24,
                timestamp: time(0),
                display: DisplayGeometry::default(),
            };
            let reduced = Frame {
                data: &sampled,
                width: columns as u32,
                height: rows as u32,
                stride: columns * 3,
                ..source
            };
            assert_eq!(vision::measure(&source), vision::measure(&reduced));
        }
    }

    #[test]
    fn prepared_frames_commit_exactly_like_the_frames_themselves() {
        let before = gallery(false);
        let after = gallery(true);
        let black = vec![0; before.len()];
        let script: [(&[u8], i64, bool); 7] = [
            (&before, 0, false),
            (&before, 3000, false),
            (&before, 90_000, true),
            (&after, 180_000, true),
            (&after, 93_000, false),
            (&black, 96_000, false),
            (&after, 180_000, true),
        ];
        for layout in [
            FeedLayout::Meeting,
            FeedLayout::Automatic,
            FeedLayout::Grid {
                columns: 2,
                rows: 1,
            },
        ] {
            let config = AnalyzerConfig {
                mode: BackendMode::Auto,
                sampling: Sampling::EveryFrame,
            };
            let mut direct = Analyzer::with_layout(config, time(-3000), layout).unwrap();
            let mut ahead = Analyzer::with_layout(config, time(-3000), layout).unwrap();
            for (pixels, pts, coarse) in script {
                let prepared = ahead.prepare(&frame(pixels, pts)).unwrap();
                let (left, right) = if coarse {
                    (
                        direct.push_coarse(frame(pixels, pts)).unwrap(),
                        ahead.push_coarse_prepared(prepared).unwrap(),
                    )
                } else {
                    direct.push(frame(pixels, pts)).unwrap();
                    ahead.push_prepared(prepared).unwrap();
                    (CoarseOutcome::Committed, CoarseOutcome::Committed)
                };
                assert_eq!(left, right, "{layout:?} at {pts}");
            }
            let json = |analyzer: &Analyzer| {
                serde_json::to_vec(&analyzer.snapshot(time(183_000)).unwrap()).unwrap()
            };
            assert_eq!(json(&direct), json(&ahead), "{layout:?}");
        }
        let mut analyzer = analyzer(Sampling::EveryFrame);
        assert!(analyzer.prepare(&frame(&[], 0)).is_err());
        let late = analyzer.prepare(&frame(&before, 0)).unwrap();
        analyzer.push(frame(&before, 3000)).unwrap();
        assert_eq!(
            analyzer.push_prepared(late).unwrap_err().kind,
            ErrorKind::InvalidTimestamp
        );
        analyzer.session = BackendSession::fake_gpu(BackendMode::GpuRequired, time(-3000));
        assert!(analyzer.prepare(&frame(&before, 6000)).is_err());
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
