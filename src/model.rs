use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt};

/// Independent version of the serialized analysis format.
pub const RESULT_SCHEMA_VERSION: u32 = 1;

/// Seconds per presentation tick. Both fields must be nonzero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeBase {
    pub numerator: u32,
    pub denominator: u32,
}

/// A source presentation timestamp, including the source clock origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    pub pts: i64,
    pub time_base: TimeBase,
}

impl Timestamp {
    /// Construct a source time. An analyzer rejects either zero time-base field.
    pub const fn new(pts: i64, numerator: u32, denominator: u32) -> Self {
        Self {
            pts,
            time_base: TimeBase {
                numerator,
                denominator,
            },
        }
    }

    /// Compare exact source times. Returns None for an invalid time base.
    pub fn compare(self, other: Self) -> Option<Ordering> {
        let (left, right) = self.common_clock(other)?;
        Some(left.cmp(&right))
    }

    pub(crate) fn common_clock(self, other: Self) -> Option<(i128, i128)> {
        if !self.is_valid() || !other.is_valid() {
            return None;
        }
        // Each product has one signed 64-bit factor and two unsigned
        // 32-bit factors, so both scaled values fit in i128.
        let left = i128::from(self.pts)
            * i128::from(self.time_base.numerator)
            * i128::from(other.time_base.denominator);
        let right = i128::from(other.pts)
            * i128::from(other.time_base.numerator)
            * i128::from(self.time_base.denominator);
        Some((left, right))
    }

    pub(crate) fn is_valid(self) -> bool {
        self.time_base.numerator != 0 && self.time_base.denominator != 0
    }

    pub(crate) fn before(self, other: Self) -> bool {
        self.compare(other) == Some(Ordering::Less)
    }
}

/// Packed 8-bit samples described in byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Gray8,
    Rgb24,
    Rgba8,
    Bgra8,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::Rgb24 => 3,
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
}

/// Clockwise rotation from coded pixels to display orientation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

/// Transform from coded pixels to display geometry. Pixel aspect applies to
/// the coded axes before rotation and is retained for sizing a display canvas.
/// Normalization cancels the pixel aspect scale for rectangle coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayGeometry {
    pub rotation: Rotation,
    pub pixel_aspect_num: u32,
    pub pixel_aspect_den: u32,
}

impl Default for DisplayGeometry {
    fn default() -> Self {
        Self {
            rotation: Rotation::R0,
            pixel_aspect_num: 1,
            pixel_aspect_den: 1,
        }
    }
}

/// Borrowed packed pixels. Alpha is ignored. The buffer must span every visible
/// pixel, including padding between rows. It can be released when push returns.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub timestamp: Timestamp,
    pub display: DisplayGeometry,
}

/// A rectangle in normalized, top-left display coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub(crate) fn overlap(self, other: Self) -> f32 {
        let width =
            ((self.x + self.width).min(other.x + other.width) - self.x.max(other.x)).max(0.0);
        let height =
            ((self.y + self.height).min(other.y + other.height) - self.y.max(other.y)).max(0.0);
        let area = width * height;
        area / (self.width * self.height + other.width * other.height - area)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileKind {
    Video,
    CameraOff,
    ScreenShare,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileRole {
    Primary,
    Thumbnail,
}

/// Identity assigned within one analysis. IDs start at one.
pub type ParticipantId = u64;
/// Contiguous visual appearance within one analysis. IDs start at one.
pub type AppearanceId = u64;

/// A less certain identity association retained for review.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityMatch {
    pub participant_id: ParticipantId,
    pub confidence: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionObservation {
    pub appearance_id: AppearanceId,
    /// Provisional when identity_uncertain is true.
    pub participant_id: Option<ParticipantId>,
    pub rect: Rect,
    pub kind: TileKind,
    pub role: TileRole,
    pub confidence: f32,
    pub identity_confidence: Option<f32>,
    pub identity_uncertain: bool,
    pub possible_matches: Vec<IdentityMatch>,
}

/// Detected regions at an actual submitted frame timestamp.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameObservation {
    pub timestamp: Timestamp,
    pub display: DisplayGeometry,
    pub confidence: f32,
    pub regions: Vec<RegionObservation>,
}

/// An identity that can own multiple overlapping or successive appearances.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParticipantTrack {
    pub id: ParticipantId,
    pub appearance_ids: Vec<AppearanceId>,
}

/// A half-open range over which one region appearance remains tracked. Query
/// frame observations for geometry because the bounds can move in this range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Appearance {
    pub id: AppearanceId,
    pub participant_id: Option<ParticipantId>,
    pub start: Timestamp,
    pub end: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Enter,
    Leave,
    Move,
    Resize,
    CameraState,
    HardCut,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub timestamp: Timestamp,
    pub participant_id: ParticipantId,
    pub appearance_id: AppearanceId,
    pub kind: ChangeKind,
    pub uncertain: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// The detector resolved a layout from a supplied frame. Individual
    /// observations may still report provisional identities or tile kinds.
    Observed,
    /// An interval between sparse samples, or beyond the last sparse sample.
    Inferred,
    /// The detector could not establish the frame's contents.
    Unknown,
    /// Input was unavailable.
    Incomplete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageInterval {
    pub start: Timestamp,
    pub end: Timestamp,
    pub status: Coverage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    /// The caller supplies every decoded frame, in presentation order.
    #[default]
    EveryFrame,
    /// The caller selectively samples frames. Between-sample geometry is inferred.
    Sparse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    /// Prefer the supported GPU, with CPU recovery on a host-readable frame.
    #[default]
    Auto,
    /// Use the CPU without probing for a GPU.
    CpuOnly,
    /// Require GPU initialization and stop on failure without committing the frame.
    GpuRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Cpu,
    Gpu,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendDiagnostic {
    pub timestamp: Timestamp,
    pub backend: Backend,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyzerConfig {
    pub mode: BackendMode,
    pub sampling: Sampling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidFrame,
    InvalidTimestamp,
    InvalidRange,
    GpuUnavailable,
    DeviceLost,
}

#[derive(Clone, Debug)]
pub struct AnalysisError {
    pub kind: ErrorKind,
    pub message: String,
    pub resume_at: Timestamp,
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AnalysisError {}

/// Versioned offline analysis. Identifiers refer to this result alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub schema_version: u32,
    pub start: Timestamp,
    pub end: Timestamp,
    pub sampling: Sampling,
    pub observations: Vec<FrameObservation>,
    pub tracks: Vec<ParticipantTrack>,
    pub appearances: Vec<Appearance>,
    pub events: Vec<ChangeEvent>,
    pub coverage: Vec<CoverageInterval>,
    pub diagnostics: Vec<BackendDiagnostic>,
    pub resume_at: Option<Timestamp>,
}

/// A serializable snapshot that borrows the analyzer's observation, track and
/// event history. Coverage and appearance ends reflect the requested scan end.
/// Drop the view before submitting another frame. Its serialized form is
/// identical to [`AnalysisResult`].
#[derive(Debug, Serialize)]
#[serde(rename = "AnalysisResult")]
pub struct AnalysisSnapshot<'a> {
    pub schema_version: u32,
    pub start: Timestamp,
    pub end: Timestamp,
    pub sampling: Sampling,
    pub observations: &'a [FrameObservation],
    pub tracks: &'a [ParticipantTrack],
    pub appearances: Vec<Appearance>,
    pub events: &'a [ChangeEvent],
    pub coverage: Vec<CoverageInterval>,
    pub diagnostics: &'a [BackendDiagnostic],
    pub resume_at: Option<Timestamp>,
}

impl AnalysisSnapshot<'_> {
    /// Copy the borrowed history into an independently owned result.
    pub fn into_owned(self) -> AnalysisResult {
        AnalysisResult {
            schema_version: self.schema_version,
            start: self.start,
            end: self.end,
            sampling: self.sampling,
            observations: self.observations.to_vec(),
            tracks: self.tracks.to_vec(),
            appearances: self.appearances,
            events: self.events.to_vec(),
            coverage: self.coverage,
            diagnostics: self.diagnostics.to_vec(),
            resume_at: self.resume_at,
        }
    }
}

impl AnalysisResult {
    /// Find the sampled frame responsible for an instant. Sparse intervals can
    /// carry a prior sample within their covered interval. Use coverage_at to
    /// determine the interval status.
    pub fn visible_at(&self, timestamp: Timestamp) -> Option<&FrameObservation> {
        if !self.in_committed_range(timestamp) {
            return None;
        }
        let interval = self.coverage_interval_at(timestamp)?;
        if interval.status == Coverage::Incomplete {
            return None;
        }
        let after = self.observations.partition_point(|frame| {
            timestamp
                .compare(frame.timestamp)
                .is_some_and(|cmp| cmp.is_ge())
        });
        self.observations
            .get(after.checked_sub(1)?)
            .filter(|frame| {
                self.in_committed_range(frame.timestamp)
                    && interval
                        .start
                        .compare(frame.timestamp)
                        .is_some_and(|cmp| cmp.is_le())
                    && frame
                        .timestamp
                        .compare(timestamp)
                        .is_some_and(|cmp| cmp.is_le())
            })
    }

    /// Return the interval status at an instant in the half-open scan range.
    pub fn coverage_at(&self, timestamp: Timestamp) -> Option<Coverage> {
        if !self.in_scan_range(timestamp) || !self.has_valid_resume() {
            return None;
        }
        if !self.in_committed_range(timestamp) {
            return Some(Coverage::Incomplete);
        }
        self.coverage_interval_at(timestamp)
            .map(|interval| interval.status)
    }

    fn coverage_interval_at(&self, timestamp: Timestamp) -> Option<&CoverageInterval> {
        if !self.in_scan_range(timestamp) {
            return None;
        }
        let index = self.coverage.partition_point(|interval| {
            interval
                .end
                .compare(timestamp)
                .is_some_and(|cmp| cmp.is_le())
        });
        self.coverage.get(index).filter(|interval| {
            interval
                .start
                .compare(timestamp)
                .is_some_and(|cmp| cmp.is_le())
                && timestamp.before(interval.end)
        })
    }

    fn in_scan_range(&self, timestamp: Timestamp) -> bool {
        self.schema_version == RESULT_SCHEMA_VERSION
            && self.start.compare(timestamp).is_some_and(|cmp| cmp.is_le())
            && timestamp.before(self.end)
    }

    fn has_valid_resume(&self) -> bool {
        self.resume_at.is_none_or(|resume| {
            self.start.compare(resume).is_some_and(|cmp| cmp.is_le())
                && resume.compare(self.end).is_some_and(|cmp| cmp.is_le())
        })
    }

    pub(crate) fn in_committed_range(&self, timestamp: Timestamp) -> bool {
        self.in_scan_range(timestamp)
            && self.has_valid_resume()
            && self.resume_at.is_none_or(|resume| timestamp.before(resume))
    }

    /// Return all appearances of a participant intersecting the half-open range.
    pub fn appearances_of(
        &self,
        participant_id: ParticipantId,
        start: Timestamp,
        end: Timestamp,
    ) -> impl Iterator<Item = &Appearance> {
        let mut lower = start;
        if lower.before(self.start) {
            lower = self.start;
        }
        let committed_end = self.resume_at.unwrap_or(self.end);
        let upper = if committed_end.before(end) {
            committed_end
        } else {
            end
        };
        let valid_range = self.in_committed_range(lower) && lower.before(upper);
        self.appearances.iter().filter(move |appearance| {
            valid_range
                && appearance.participant_id == Some(participant_id)
                && self.in_committed_range(appearance.start)
                && appearance.start.before(appearance.end)
                && appearance
                    .end
                    .compare(committed_end)
                    .is_some_and(|cmp| cmp.is_le())
                && appearance.start.before(upper)
                && lower.before(appearance.end)
        })
    }
}
