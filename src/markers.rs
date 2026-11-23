use crate::{
    AnalysisResult, ChangeEvent, ChangeKind, Coverage, FrameObservation, ParticipantId,
    RegionObservation, TileKind, Timestamp, vision::Tile,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerUnit {
    Participants,
    Slots,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerThreshold {
    Any,
    AtLeast(u32),
    All,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerRule {
    pub unit: MarkerUnit,
    pub threshold: MarkerThreshold,
    pub kinds: Vec<ChangeKind>,
    /// None selects the full roster. An empty list selects nobody.
    pub roster: Option<Vec<ParticipantId>>,
    /// Fixed window measured from the first selected change in each group.
    pub settling_window_ms: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneMarker {
    pub timestamp: Timestamp,
    pub end_timestamp: Timestamp,
    pub changed_count: usize,
    /// Number of selected participants or occupied slots immediately before or after the group.
    pub eligible_count: usize,
    pub uncertain: bool,
    /// Affected participants with selected events in this group.
    pub participant_ids: Vec<ParticipantId>,
}

impl AnalysisResult {
    /// Apply a marker rule to stored events and their surrounding frame observations.
    pub fn scene_markers(&self, rule: &MarkerRule) -> Vec<SceneMarker> {
        let roster = rule
            .roster
            .as_ref()
            .map(|ids| ids.iter().copied().collect());
        let selected = |id| includes(&roster, id);
        let Some(baseline) = self.observations.first() else {
            return Vec::new();
        };
        let events: Vec<_> = self
            .events
            .iter()
            .filter(|event| {
                baseline.timestamp.before(event.timestamp)
                    && event.timestamp.before(self.end)
                    && rule.kinds.contains(&event.kind)
                    && selected(event.participant_id)
            })
            .collect();

        let mut markers = Vec::new();
        let mut pending = events.as_slice();
        while let Some(first) = pending.first() {
            let count = pending.partition_point(|event| {
                within_window(first.timestamp, event.timestamp, rule.settling_window_ms)
            });
            let (group, rest) = pending.split_at(count);
            pending = rest;
            let timestamp = first.timestamp;
            let end_timestamp = group[group.len() - 1].timestamp;
            let before_index = self
                .observations
                .partition_point(|frame| frame.timestamp.before(timestamp));
            let after_index = self
                .observations
                .partition_point(|frame| !end_timestamp.before(frame.timestamp));
            let Some(frames) = before_index
                .checked_sub(1)
                .and_then(|before| self.observations.get(before..after_index))
            else {
                continue;
            };
            let (Some(before), Some(after)) = (frames.first(), frames.last()) else {
                continue;
            };

            let participant_ids: BTreeSet<_> =
                group.iter().map(|event| event.participant_id).collect();
            let counts = match rule.unit {
                MarkerUnit::Participants => {
                    let eligible: BTreeSet<_> = before
                        .regions
                        .iter()
                        .chain(&after.regions)
                        .filter_map(participant)
                        .filter(|&id| selected(id))
                        .collect();
                    (
                        participant_ids.len(),
                        eligible.len(),
                        !eligible.is_empty() && eligible.is_subset(&participant_ids),
                    )
                }
                MarkerUnit::Slots => {
                    let Some(counts) = slot_changes(frames, group, &roster) else {
                        continue;
                    };
                    counts
                }
            };
            let (changed_count, eligible_count, all) = counts;
            let matches = match rule.threshold {
                MarkerThreshold::Any => changed_count > 0,
                MarkerThreshold::AtLeast(count) => {
                    changed_count > 0 && changed_count >= count as usize
                }
                MarkerThreshold::All => all,
            };
            if matches {
                markers.push(SceneMarker {
                    timestamp,
                    end_timestamp,
                    changed_count,
                    eligible_count,
                    uncertain: group.iter().any(|event| event.uncertain)
                        || self.marker_coverage_uncertain(before.timestamp, end_timestamp)
                        || before.regions.iter().chain(&after.regions).any(|region| {
                            region.identity_uncertain
                                && (participant(region).is_some_and(selected)
                                    || region
                                        .possible_matches
                                        .iter()
                                        .any(|candidate| selected(candidate.participant_id)))
                        }),
                    participant_ids: participant_ids.into_iter().collect(),
                });
            }
        }
        markers
    }

    fn marker_coverage_uncertain(&self, from: Timestamp, through: Timestamp) -> bool {
        let start = self
            .coverage
            .partition_point(|interval| !from.before(interval.end));
        let mut covered_until = from;
        for interval in self.coverage[start..]
            .iter()
            .take_while(|interval| !through.before(interval.start))
        {
            if covered_until.before(interval.start) || interval.status != Coverage::Observed {
                return true;
            }
            covered_until = interval.end;
        }
        !through.before(covered_until)
    }
}

fn includes(roster: &Option<BTreeSet<ParticipantId>>, id: ParticipantId) -> bool {
    roster.as_ref().is_none_or(|ids| ids.contains(&id))
}

fn participant(region: &RegionObservation) -> Option<ParticipantId> {
    (region.kind != TileKind::ScreenShare)
        .then_some(region.participant_id)
        .flatten()
}

fn within_window(first: Timestamp, next: Timestamp, window_ms: u32) -> bool {
    let left = i128::from(first.pts)
        * i128::from(first.time_base.numerator)
        * i128::from(next.time_base.denominator);
    let right = i128::from(next.pts)
        * i128::from(next.time_base.numerator)
        * i128::from(first.time_base.denominator);
    let limit = i128::from(window_ms)
        * i128::from(first.time_base.denominator)
        * i128::from(next.time_base.denominator)
        / 1000;
    right
        .checked_sub(left)
        .is_some_and(|elapsed| (0..=limit).contains(&elapsed))
}

/// Match persistent geometric slots while allowing small changes to detected bounds.
fn comparable_regions<'a>(
    baseline: &FrameObservation,
    frame: &'a FrameObservation,
) -> Option<Vec<&'a RegionObservation>> {
    if baseline.display != frame.display || baseline.regions.len() != frame.regions.len() {
        return None;
    }
    let mut used = BTreeSet::new();
    baseline
        .regions
        .iter()
        .map(|slot| {
            let mut matches = frame.regions.iter().enumerate().filter(|(_, region)| {
                slot.role == region.role
                    && slot.rect.overlap(region.rect) >= 0.80
                    && (slot.kind == TileKind::ScreenShare)
                        == (region.kind == TileKind::ScreenShare)
            });
            let (index, region) = matches.next()?;
            (matches.next().is_none() && used.insert(index)).then_some(region)
        })
        .collect()
}

fn slot_changes(
    frames: &[FrameObservation],
    mut events: &[&ChangeEvent],
    roster: &Option<BTreeSet<ParticipantId>>,
) -> Option<(usize, usize, bool)> {
    let baseline = &frames[0];
    let selected =
        |region: &RegionObservation| participant(region).filter(|&id| includes(roster, id));
    let mut previous: Vec<_> = baseline.regions.iter().collect();
    let mut changed = BTreeSet::new();
    for frame in &frames[1..] {
        let current = comparable_regions(baseline, frame)?;
        let count = events.partition_point(|event| !frame.timestamp.before(event.timestamp));
        let (transition, rest) = events.split_at(count);
        events = rest;
        for (slot, (old, new)) in previous.iter().zip(&current).enumerate() {
            let old_id = selected(old);
            let new_id = selected(new);
            if old_id.is_none() && new_id.is_none() {
                continue;
            }
            let relevant = transition.iter().any(|event| {
                if event
                    .timestamp
                    .compare(frame.timestamp)
                    .is_none_or(|cmp| !cmp.is_eq())
                {
                    return false;
                }
                let matches_old = old_id == Some(event.participant_id);
                let matches_new = new_id == Some(event.participant_id);
                match event.kind {
                    // Enter and leave describe the participant, even when they
                    // occupy several slots at the same time.
                    ChangeKind::Enter => old_id != new_id && matches_new,
                    ChangeKind::Leave => old_id != new_id && matches_old,
                    _ => {
                        (matches_old && old.appearance_id == event.appearance_id)
                            || (matches_new && new.appearance_id == event.appearance_id)
                    }
                }
            });
            if relevant {
                changed.insert(slot);
            }
        }
        previous = current;
    }
    let eligible: BTreeSet<_> = baseline
        .regions
        .iter()
        .zip(previous)
        .enumerate()
        .filter_map(|(slot, (old, new))| {
            (selected(old).is_some() || selected(new).is_some()).then_some(slot)
        })
        .collect();
    Some((
        changed.len(),
        eligible.len(),
        !eligible.is_empty() && eligible.is_subset(&changed),
    ))
}

/// Look for a broad picture discontinuity in a stationary, continuously
/// associated video tile. The central cells prevent border changes from
/// dominating the decision.
pub(crate) fn hard_cut(previous: &Tile, current: &Tile) -> bool {
    if previous.kind != TileKind::Video
        || current.kind != TileKind::Video
        || previous.role != current.role
        || previous.rect.overlap(current.rect) < 0.93
    {
        return false;
    }
    let Some(previous_cells) = previous.descriptor.get(..48) else {
        return false;
    };
    let Some(current_cells) = current.descriptor.get(..48) else {
        return false;
    };
    let mut changed = 0;
    let mut changed_center = 0;
    for (cell, (old, new)) in previous_cells
        .chunks_exact(3)
        .zip(current_cells.chunks_exact(3))
        .enumerate()
    {
        let mean = old.iter().zip(new).map(|(a, b)| (a - b).abs()).sum::<f32>() / 3.0;
        if mean >= 0.13 {
            changed += 1;
            changed_center += usize::from(matches!(cell, 5 | 6 | 9 | 10));
        }
    }
    changed >= 10 && changed_center >= 3
}
