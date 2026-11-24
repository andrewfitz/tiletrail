use crate::{AnalysisError, AnalysisResult, Analyzer, ErrorKind, Timestamp, error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Ready,
    Analyzing,
    NeedsReplay,
    Complete,
}

/// Source timeline progress. Counts committed observations, including in sparse mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisProgress {
    pub state: ProgressState,
    pub frames_processed: usize,
    pub last_timestamp: Option<Timestamp>,
    /// Whole percent of the requested time range. Unknown when no expected end was supplied.
    pub percent: Option<u8>,
}

pub(super) fn validate_scan_range(
    start: Timestamp,
    last: Option<Timestamp>,
    resume_at: Option<Timestamp>,
    end: Timestamp,
) -> Result<(), AnalysisError> {
    if !start.before(end)
        || last.is_some_and(|t| t.before(start) || !t.before(end))
        || resume_at.is_some_and(|t| {
            !t.is_valid()
                || t.before(start)
                || end.before(t)
                || last.is_some_and(|last| !last.before(t))
        })
    {
        return Err(error(
            ErrorKind::InvalidRange,
            "scan end must follow the start and committed frames, and include any pending frame",
            resume_at.unwrap_or(end),
        ));
    }
    Ok(())
}

// Compares scaled timestamp differences without losing the tick immediately before a
// whole-percent boundary. Mixed time bases can require up to 167 bits for the product.
fn wide_product(value: u128, factor: u64) -> (u128, u64) {
    let low = u128::from(value as u64) * u128::from(factor);
    let high = (value >> 64) * u128::from(factor) + (low >> 64);
    (high, low as u64)
}

fn committed_percent(start: Timestamp, last: Option<Timestamp>, end: Timestamp) -> u8 {
    let Some(last) = last else { return 0 };
    let (start_ticks, last_ticks) = start.common_clock(last).expect("validated timestamps");
    let elapsed = last_ticks.abs_diff(start_ticks);
    let (start_ticks, end_ticks) = start.common_clock(end).expect("validated timestamps");
    let duration = end_ticks.abs_diff(start_ticks);
    // The start denominator is common to both differences and cancels.
    let numerator_scale = 100 * u64::from(end.time_base.denominator);
    let denominator_scale = u64::from(last.time_base.denominator);
    if let (Some(numerator), Some(denominator)) = (
        elapsed.checked_mul(u128::from(numerator_scale)),
        duration.checked_mul(u128::from(denominator_scale)),
    ) {
        return (numerator / denominator) as u8;
    }
    let numerator = wide_product(elapsed, numerator_scale);
    let mut floor = 0;
    let mut ceiling = 100;
    while ceiling - floor > 1 {
        let middle = (floor + ceiling) / 2;
        if wide_product(duration, denominator_scale * u64::from(middle)) <= numerator {
            floor = middle;
        } else {
            ceiling = middle;
        }
    }
    floor
}

impl Analyzer {
    /// Poll between pushes. A snapshot can be taken without ending the stream.
    /// Supply the exclusive source end to get a percentage, or None for an open stream.
    pub fn progress(
        &self,
        expected_end: Option<Timestamp>,
    ) -> Result<AnalysisProgress, AnalysisError> {
        let observations = &self.tracker.observations;
        let last = observations.last().map(|frame| frame.timestamp);
        let percent = if let Some(end) = expected_end {
            validate_scan_range(self.start, last, self.uncommitted, end)?;
            Some(committed_percent(self.start, last, end))
        } else {
            None
        };
        let state = if self.uncommitted.is_some() {
            ProgressState::NeedsReplay
        } else if last.is_some() {
            ProgressState::Analyzing
        } else {
            ProgressState::Ready
        };
        Ok(AnalysisProgress {
            state,
            frames_processed: observations.len(),
            last_timestamp: last,
            percent,
        })
    }
}

impl AnalysisResult {
    /// Progress of this finished range. An interrupted snapshot reports its last committed frame.
    pub fn progress(&self) -> Result<AnalysisProgress, AnalysisError> {
        let last = self.observations.last().map(|frame| frame.timestamp);
        validate_scan_range(self.start, last, self.resume_at, self.end)?;
        let complete = self.resume_at.is_none();
        Ok(AnalysisProgress {
            state: if complete {
                ProgressState::Complete
            } else {
                ProgressState::NeedsReplay
            },
            frames_processed: self.observations.len(),
            last_timestamp: last,
            percent: Some(if complete {
                100
            } else {
                committed_percent(self.start, last, self.end)
            }),
        })
    }
}
