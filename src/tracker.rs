use crate::markers::hard_cut;
use crate::vision::{Detection, Tile, appearance_distance};
use crate::*;
use pathfinding::{kuhn_munkres::kuhn_munkres, matrix::Matrix};
use std::collections::BTreeMap;

const UNMATCHED: i32 = 540;
const REVIEW_MATCH: i32 = 440;
const AMBIGUITY_MARGIN: i32 = 45;
const DORMANT_LIMIT: usize = 128;

#[derive(Clone)]
struct Sample {
    appearance: AppearanceId,
    participant: Option<ParticipantId>,
    tile: Tile,
    uncertain: bool,
    alternatives: Vec<IdentityMatch>,
    last_video: Option<Vec<f32>>,
}

struct Decision {
    matched: Option<usize>,
    possible: Vec<IdentityMatch>,
    ambiguous: bool,
}

#[derive(Default)]
pub(crate) struct Tracker {
    pub(crate) observations: Vec<FrameObservation>,
    pub(crate) tracks: Vec<ParticipantTrack>,
    pub(crate) appearances: Vec<Appearance>,
    pub(crate) events: Vec<ChangeEvent>,
    active: Vec<Sample>,
    dormant: BTreeMap<ParticipantId, Sample>,
    uncertain_gap: bool,
    has_baseline: bool,
}

impl Tracker {
    pub(crate) fn push(&mut self, frame: &Frame<'_>, detection: Detection, sparse: bool) {
        let timestamp = frame.timestamp;
        if detection.confidence < MIN_LAYOUT_CONFIDENCE {
            self.close_active(timestamp);
            self.uncertain_gap = true;
            self.observations.push(FrameObservation {
                timestamp,
                display: frame.display,
                confidence: detection.confidence,
                regions: Vec::new(),
            });
            return;
        }

        let tiles = detection.tiles;
        let active_count = self.active.len();
        let candidates: Vec<&Sample> = self.active.iter().chain(self.dormant.values()).collect();
        let mut weights = Matrix::new(tiles.len(), candidates.len() + tiles.len(), UNMATCHED);
        for (row, tile) in tiles.iter().enumerate() {
            for (col, candidate) in candidates.iter().enumerate() {
                weights[(row, col)] = score(tile, candidate, col < active_count);
            }
        }
        let (_, matches) = if tiles.is_empty() {
            (0, Vec::new())
        } else {
            kuhn_munkres(&weights)
        };

        let mut decisions: Vec<_> = matches
            .iter()
            .enumerate()
            .map(|(row, &column)| {
                let matched = (column < candidates.len() && weights[(row, column)] > UNMATCHED)
                    .then_some(column);
                let mut possible: Vec<IdentityMatch> = Vec::new();
                for (index, candidate) in candidates.iter().enumerate() {
                    let Some(participant_id) = candidate.participant else {
                        continue;
                    };
                    let weight = weights[(row, index)];
                    if weight <= REVIEW_MATCH {
                        continue;
                    }
                    let confidence = (weight as f32 / 1000.0).min(1.0);
                    if let Some(previous) = possible
                        .iter_mut()
                        .find(|previous| previous.participant_id == participant_id)
                    {
                        previous.confidence = previous.confidence.max(confidence);
                    } else {
                        possible.push(IdentityMatch {
                            participant_id,
                            confidence,
                        });
                    }
                }
                possible.sort_by(|a, b| {
                    b.confidence
                        .total_cmp(&a.confidence)
                        .then(a.participant_id.cmp(&b.participant_id))
                });
                let ambiguous = match matched {
                    Some(index) => candidates[index].participant.is_some_and(|participant| {
                        possible.iter().any(|option| {
                            option.participant_id != participant
                                && option.confidence + AMBIGUITY_MARGIN as f32 / 1000.0
                                    >= weights[(row, index)] as f32 / 1000.0
                        })
                    }),
                    None => !possible.is_empty(),
                };
                Decision {
                    matched: matched.filter(|_| !ambiguous),
                    possible,
                    ambiguous,
                }
            })
            .collect();

        review_placeholders(&tiles, &candidates, active_count, &mut decisions);

        let mut next = Vec::with_capacity(tiles.len());
        let mut regions = Vec::with_capacity(tiles.len());
        for (row, decision) in decisions.iter().enumerate() {
            let matched = decision.matched;
            let tile = &tiles[row];
            let candidate = matched.map(|index| candidates[index]);
            let duplicate = (candidate.is_none() && tile.kind == TileKind::Video)
                .then(|| simultaneous_identity(row, &tiles, &decisions, &candidates, &next))
                .flatten();
            let identity_confidence = matched.and_then(|index| {
                candidates[index]
                    .participant
                    .map(|_| (weights[(row, index)] as f32 / 1000.0).min(1.0))
            });
            let participant = if tile.kind == TileKind::ScreenShare {
                None
            } else if let Some(candidate) = candidate {
                candidate.participant
            } else {
                Some(duplicate.unwrap_or_else(|| Self::new_participant(&mut self.tracks)))
            };
            let appearance = match matched.filter(|&index| index < active_count) {
                Some(index) => candidates[index].appearance,
                None => Self::new_appearance(
                    &mut self.tracks,
                    &mut self.appearances,
                    participant,
                    timestamp,
                ),
            };
            let uncertain = candidate.is_some_and(|sample| sample.uncertain)
                || (duplicate.is_none() && decision.ambiguous)
                || tile.kind == TileKind::Unknown;
            let mut possible_matches = decision.possible.clone();
            if let Some(candidate) = candidate.filter(|sample| sample.uncertain) {
                for alternative in &candidate.alternatives {
                    if !possible_matches
                        .iter()
                        .any(|option| option.participant_id == alternative.participant_id)
                    {
                        possible_matches.push(alternative.clone());
                    }
                }
            }
            let alternatives = if uncertain {
                possible_matches
                    .iter()
                    .filter(|option| Some(option.participant_id) != participant)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let last_video = if tile.kind == TileKind::Video {
                None
            } else {
                candidate.and_then(|sample| {
                    if sample.tile.kind == TileKind::Video {
                        Some(sample.tile.descriptor.clone())
                    } else {
                        sample.last_video.clone()
                    }
                })
            };
            next.push(Sample {
                appearance,
                participant,
                tile: tile.clone(),
                uncertain,
                alternatives,
                last_video,
            });
            regions.push(RegionObservation {
                appearance_id: appearance,
                participant_id: participant,
                rect: tile.rect,
                kind: tile.kind,
                role: tile.role,
                confidence: tile.confidence,
                identity_confidence: identity_confidence.filter(|_| !uncertain),
                identity_uncertain: uncertain,
                possible_matches,
            });
        }

        if self.has_baseline {
            self.record_events(timestamp, &next, sparse || self.uncertain_gap);
        }
        let previous = std::mem::take(&mut self.active);
        for sample in previous {
            if !next
                .iter()
                .any(|current| current.appearance == sample.appearance)
            {
                self.retire(sample, timestamp);
            }
        }
        for participant in next.iter().filter_map(|sample| sample.participant) {
            self.dormant.remove(&participant);
        }
        self.active = next;
        self.uncertain_gap = false;
        self.has_baseline = true;
        self.observations.push(FrameObservation {
            timestamp,
            display: frame.display,
            confidence: detection.confidence,
            regions,
        });
    }

    fn new_participant(tracks: &mut Vec<ParticipantTrack>) -> ParticipantId {
        let id = tracks.len() as u64 + 1;
        tracks.push(ParticipantTrack {
            id,
            appearance_ids: Vec::new(),
        });
        id
    }

    fn new_appearance(
        tracks: &mut [ParticipantTrack],
        appearances: &mut Vec<Appearance>,
        participant_id: Option<ParticipantId>,
        timestamp: Timestamp,
    ) -> AppearanceId {
        let id = appearances.len() as u64 + 1;
        appearances.push(Appearance {
            id,
            participant_id,
            start: timestamp,
            end: timestamp,
        });
        if let Some(participant) = participant_id {
            tracks[participant as usize - 1].appearance_ids.push(id);
        }
        id
    }

    fn record_events(&mut self, timestamp: Timestamp, next: &[Sample], uncertain: bool) {
        for prior in &self.active {
            let Some(participant_id) = prior.participant else {
                continue;
            };
            let mut emit = |kind, uncertain| {
                self.events.push(ChangeEvent {
                    timestamp,
                    participant_id,
                    appearance_id: prior.appearance,
                    kind,
                    uncertain,
                });
            };
            let Some(current) = next
                .iter()
                .find(|sample| sample.appearance == prior.appearance)
            else {
                if !next
                    .iter()
                    .any(|current| current.participant == prior.participant)
                    && self
                        .active
                        .iter()
                        .find(|sample| sample.participant == prior.participant)
                        .is_some_and(|sample| sample.appearance == prior.appearance)
                {
                    emit(
                        ChangeKind::Leave,
                        uncertain
                            || prior.uncertain
                            || next.iter().any(|sample| could_be(sample, participant_id)),
                    );
                }
                continue;
            };
            let uncertain = uncertain || prior.uncertain || current.uncertain;
            let a = prior.tile.rect;
            let b = current.tile.rect;
            if (a.x + a.width * 0.5 - b.x - b.width * 0.5).abs() > 0.025
                || (a.y + a.height * 0.5 - b.y - b.height * 0.5).abs() > 0.025
            {
                emit(ChangeKind::Move, uncertain);
            }
            if (a.width - b.width).abs() > 0.025 || (a.height - b.height).abs() > 0.025 {
                emit(ChangeKind::Resize, uncertain);
            }
            if matches!(
                (prior.tile.kind, current.tile.kind),
                (TileKind::Video, TileKind::CameraOff) | (TileKind::CameraOff, TileKind::Video)
            ) {
                emit(ChangeKind::CameraState, uncertain);
            } else if hard_cut(&prior.tile, &current.tile) {
                emit(ChangeKind::HardCut, true);
            }
        }
        for current in next {
            let Some(participant_id) = current.participant else {
                continue;
            };
            if !self
                .active
                .iter()
                .any(|sample| sample.participant == current.participant)
                && next
                    .iter()
                    .find(|sample| sample.participant == current.participant)
                    .is_some_and(|sample| sample.appearance == current.appearance)
            {
                self.events.push(ChangeEvent {
                    timestamp,
                    participant_id,
                    appearance_id: current.appearance,
                    kind: ChangeKind::Enter,
                    uncertain: uncertain
                        || current.uncertain
                        || self
                            .active
                            .iter()
                            .any(|sample| could_be(sample, participant_id)),
                });
            }
        }
    }

    fn close_active(&mut self, timestamp: Timestamp) {
        let previous = std::mem::take(&mut self.active);
        for sample in previous {
            self.retire(sample, timestamp);
        }
    }

    fn retire(&mut self, sample: Sample, timestamp: Timestamp) {
        self.appearances[sample.appearance as usize - 1].end = timestamp;
        if let Some(id) = sample.participant.filter(|_| !sample.uncertain) {
            let prefer_existing = self.dormant.get(&id).is_some_and(|known| {
                known.tile.kind == TileKind::Video
                    && (sample.tile.kind != TileKind::Video || known.tile.role == TileRole::Primary)
            });
            if !prefer_existing {
                self.dormant.insert(id, sample);
            }
            if self.dormant.len() > DORMANT_LIMIT {
                let oldest = self.dormant.iter().min_by(|a, b| {
                    self.appearances[a.1.appearance as usize - 1]
                        .end
                        .compare(self.appearances[b.1.appearance as usize - 1].end)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                if let Some((&id, _)) = oldest {
                    self.dormant.remove(&id);
                }
            }
        }
    }

    pub(crate) fn appearances_at_end(&self, end: Timestamp) -> Vec<Appearance> {
        let mut appearances = self.appearances.clone();
        for sample in &self.active {
            appearances[sample.appearance as usize - 1].end = end;
        }
        appearances
    }
}

fn could_be(sample: &Sample, participant: ParticipantId) -> bool {
    sample.uncertain
        && (sample.tile.kind == TileKind::Unknown
            || sample
                .alternatives
                .iter()
                .any(|option| option.participant_id == participant))
}

/// A camera-off card provides no visual link to a missing video. Across a
/// reflow, the previously unseen card could represent any absent participant.
/// Exclude feeds that were actually recognized elsewhere in the frame.
fn review_placeholders(
    tiles: &[Tile],
    candidates: &[&Sample],
    active_count: usize,
    decisions: &mut [Decision],
) {
    if !tiles.iter().any(|tile| tile.kind == TileKind::CameraOff) {
        return;
    }
    let mut visible = std::collections::BTreeSet::new();
    let mut reflow = false;
    for (tile, decision) in tiles.iter().zip(decisions.iter()) {
        if tile.kind != TileKind::Video {
            continue;
        }
        if let Some(index) = decision.matched {
            let prior = candidates[index];
            if !prior.uncertain {
                visible.extend(prior.participant);
                reflow |= index < active_count && prior.tile.rect.overlap(tile.rect) < 0.5;
            }
        }
    }

    for (tile, decision) in tiles.iter().zip(decisions) {
        if tile.kind != TileKind::CameraOff || (decision.matched.is_some() && !reflow) {
            continue;
        }
        // A provisional card remaining in its slot already carries its alternatives.
        if decision.matched.is_some_and(|i| candidates[i].uncertain) {
            continue;
        }
        let mut options = BTreeMap::new();
        for candidate in candidates {
            if let Some(id) = candidate.participant.filter(|id| !visible.contains(id)) {
                // Equal, review-only prior for visually indistinguishable possibilities.
                options.insert(id, (REVIEW_MATCH + 1) as f32 / 1000.0);
            }
        }
        if !options.is_empty() && (decision.matched.is_none() || options.len() > 1) {
            for entry in &decision.possible {
                if let Some(confidence) = options.get_mut(&entry.participant_id) {
                    *confidence = confidence.max(entry.confidence);
                }
            }
            decision.possible = options
                .into_iter()
                .map(|(participant_id, confidence)| IdentityMatch {
                    participant_id,
                    confidence,
                })
                .collect();
            decision
                .possible
                .sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
            decision.matched = None;
            decision.ambiguous = true;
        }
    }
}

fn score(tile: &Tile, candidate: &Sample, active: bool) -> i32 {
    let prior = &candidate.tile;
    let spatial = tile.rect.overlap(prior.rect);
    if tile.kind == TileKind::ScreenShare || prior.kind == TileKind::ScreenShare {
        return if active && tile.kind == prior.kind {
            (1000.0 * spatial) as i32
        } else {
            0
        };
    }
    if tile.kind == TileKind::Video && prior.kind != TileKind::Video {
        if let Some(remembered) = &candidate.last_video {
            let appearance = visual_score(&tile.descriptor, remembered);
            let confidence = if active {
                0.83 * appearance + 0.17 * spatial
            } else {
                0.76 * appearance
            };
            let remembered_score = (confidence * 1000.0) as i32;
            // A slot can suggest continuity after the camera comes back on,
            // but conflicting remembered video makes it a review candidate.
            return if remembered_score <= UNMATCHED && active && spatial >= 0.9 {
                UNMATCHED
            } else {
                remembered_score
            };
        }
    }
    if tile.kind == TileKind::Unknown || prior.kind == TileKind::Unknown {
        return if active { (650.0 * spatial) as i32 } else { 0 };
    }
    if tile.kind == TileKind::CameraOff || prior.kind == TileKind::CameraOff {
        return if active && spatial >= 0.9 {
            (650.0 * spatial) as i32
        } else if tile.kind == TileKind::CameraOff && prior.kind == TileKind::CameraOff {
            // Placeholder geometry can rule out continuity, but a returning
            // generic card does not identify its participant.
            (450.0 + 70.0 * spatial) as i32
        } else {
            0
        };
    }
    let visual = visual_score(&tile.descriptor, &prior.descriptor);
    let confidence = if active {
        0.83 * visual + 0.17 * spatial
    } else {
        0.76 * visual
    };
    (confidence * 1000.0) as i32
}

fn visual_score(left: &[f32], right: &[f32]) -> f32 {
    (1.0 - appearance_distance(left, right) * 3.0).max(0.0)
}

fn simultaneous_identity(
    row: usize,
    tiles: &[Tile],
    decisions: &[Decision],
    candidates: &[&Sample],
    assigned: &[Sample],
) -> Option<ParticipantId> {
    let tile = &tiles[row];
    if tile.kind != TileKind::Video {
        return None;
    }
    let mut compatible = tiles
        .iter()
        .enumerate()
        .filter(|(_, other)| same_feed(tile, other));
    let (index, _) = compatible.next()?;
    if compatible.next().is_some()
        || tiles
            .iter()
            .filter(|other| same_feed(&tiles[index], other))
            .count()
            != 1
    {
        return None;
    }
    if let Some(sample) = assigned.get(index) {
        return (!sample.uncertain).then_some(sample.participant).flatten();
    }
    decisions[index].matched.and_then(|index| {
        let candidate = candidates[index];
        (!candidate.uncertain)
            .then_some(candidate.participant)
            .flatten()
    })
}

fn same_feed(tile: &Tile, other: &Tile) -> bool {
    other.kind == TileKind::Video
        && other.role != tile.role
        && other.rect.overlap(tile.rect) < 0.1
        && appearance_distance(&tile.descriptor, &other.descriptor) < 0.035
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: f32, y: f32, side: f32, identity: f32) -> Tile {
        Tile {
            rect: Rect {
                x,
                y,
                width: side,
                height: side,
            },
            role: TileRole::Primary,
            kind: TileKind::Video,
            descriptor: vec![identity; 72],
            confidence: 1.0,
        }
    }

    fn push(tracker: &mut Tracker, pts: i64, tiles: Vec<Tile>) {
        let timestamp = Timestamp::new(pts, 1, 30);
        tracker.push(
            &Frame {
                data: &[],
                width: 0,
                height: 0,
                stride: 0,
                format: PixelFormat::Gray8,
                timestamp,
                display: DisplayGeometry::default(),
            },
            Detection {
                tiles,
                confidence: 1.0,
            },
            false,
        );
    }

    #[test]
    fn reflow_assigns_visual_identity_and_preserves_appearance() {
        let mut tracker = Tracker::default();
        push(
            &mut tracker,
            0,
            vec![tile(0.0, 0.0, 0.4, 0.1), tile(0.5, 0.0, 0.4, 0.8)],
        );
        push(
            &mut tracker,
            1,
            vec![tile(0.0, 0.5, 0.5, 0.8), tile(0.5, 0.5, 0.5, 0.1)],
        );
        let first = &tracker.observations[0].regions;
        let second = &tracker.observations[1].regions;
        assert_eq!(first[0].participant_id, second[1].participant_id);
        assert_eq!(first[0].appearance_id, second[1].appearance_id);
        assert_eq!(first[1].participant_id, second[0].participant_id);
        assert_eq!(first[1].appearance_id, second[0].appearance_id);
        assert_eq!(
            tracker
                .events
                .iter()
                .filter(|event| event.kind == ChangeKind::Move)
                .count(),
            2
        );
        assert_eq!(
            tracker
                .events
                .iter()
                .filter(|event| event.kind == ChangeKind::Resize)
                .count(),
            2
        );
    }

    #[test]
    fn records_return_as_new_appearance_on_existing_track() {
        let mut tracker = Tracker::default();
        push(&mut tracker, 0, vec![tile(0.0, 0.0, 0.4, 0.1)]);
        push(&mut tracker, 1, vec![]);
        push(&mut tracker, 2, vec![tile(0.6, 0.0, 0.4, 0.1)]);
        assert_eq!(tracker.tracks.len(), 1);
        assert_eq!(tracker.tracks[0].appearance_ids, [1, 2]);
        assert_eq!(tracker.appearances[0].end, Timestamp::new(1, 1, 30));
        let events: Vec<_> = tracker.events.iter().map(|event| event.kind).collect();
        assert_eq!(events, [ChangeKind::Leave, ChangeKind::Enter]);
    }

    #[test]
    fn retains_video_evidence_through_camera_off_and_absence() {
        let mut tracker = Tracker::default();
        let video = tile(0.0, 0.0, 0.4, 0.1);
        let mut off = tile(0.0, 0.0, 0.4, 0.8);
        off.kind = TileKind::CameraOff;
        push(&mut tracker, 0, vec![video.clone()]);
        push(&mut tracker, 1, vec![off.clone()]);
        push(&mut tracker, 2, vec![off]);
        push(&mut tracker, 3, vec![]);
        push(&mut tracker, 4, vec![video]);
        assert_eq!(tracker.tracks.len(), 1);
        let events: Vec<_> = tracker.events.iter().map(|event| event.kind).collect();
        assert_eq!(
            events,
            [
                ChangeKind::CameraState,
                ChangeKind::Leave,
                ChangeKind::Enter
            ]
        );
    }

    #[test]
    fn conflicting_video_after_camera_off_keeps_prior_identity_as_candidate() {
        let mut tracker = Tracker::default();
        let alice = tile(0.0, 0.0, 0.4, 0.1);
        let mut off = alice.clone();
        off.kind = TileKind::CameraOff;
        let different_feed = tile(0.0, 0.0, 0.4, 0.85);
        push(&mut tracker, 0, vec![alice]);
        push(&mut tracker, 1, vec![off]);
        push(&mut tracker, 2, vec![different_feed]);
        let current = &tracker.observations[2].regions[0];
        assert_ne!(current.participant_id, Some(1));
        assert!(current.identity_uncertain);
        assert!(current.identity_confidence.is_none());
        assert!(
            current
                .possible_matches
                .iter()
                .any(|option| option.participant_id == 1)
        );
        let transition: Vec<_> = tracker
            .events
            .iter()
            .filter(|event| event.timestamp == Timestamp::new(2, 1, 30))
            .map(|event| (event.kind, event.uncertain))
            .collect();
        assert_eq!(
            transition,
            [(ChangeKind::Leave, true), (ChangeKind::Enter, true)]
        );
    }

    #[test]
    fn equally_plausible_returns_have_provisional_identity_and_candidates() {
        let mut tracker = Tracker::default();
        push(
            &mut tracker,
            0,
            vec![tile(0.0, 0.0, 0.4, 0.2), tile(0.5, 0.0, 0.4, 0.2)],
        );
        push(&mut tracker, 1, vec![]);
        push(&mut tracker, 2, vec![tile(0.0, 0.5, 0.4, 0.2)]);
        push(&mut tracker, 3, vec![tile(0.0, 0.5, 0.4, 0.2)]);
        let returned = &tracker.observations[2].regions[0];
        let continued = &tracker.observations[3].regions[0];
        assert_eq!(returned.participant_id, Some(3));
        assert_eq!(returned.participant_id, continued.participant_id);
        for region in [returned, continued] {
            assert!(region.identity_uncertain);
            assert!(region.identity_confidence.is_none());
            assert!(
                region
                    .possible_matches
                    .iter()
                    .any(|candidate| candidate.participant_id == 1)
            );
            assert!(
                region
                    .possible_matches
                    .iter()
                    .any(|candidate| candidate.participant_id == 2)
            );
        }
    }

    #[test]
    fn one_participant_can_hold_primary_and_thumbnail() {
        let mut tracker = Tracker::default();
        let stage = tile(0.0, 0.0, 0.65, 0.2);
        let mut thumbnail = tile(0.75, 0.6, 0.25, 0.2);
        thumbnail.role = TileRole::Thumbnail;
        let mut share = tile(0.0, 0.0, 0.75, 0.95);
        share.kind = TileKind::ScreenShare;
        push(&mut tracker, 0, vec![stage.clone(), thumbnail.clone()]);
        push(&mut tracker, 1, vec![share, thumbnail]);
        let first = &tracker.observations[0].regions;
        let second = &tracker.observations[1].regions;
        assert_eq!(first[0].participant_id, first[1].participant_id);
        assert_ne!(first[0].appearance_id, first[1].appearance_id);
        assert_eq!(second[0].participant_id, None);
        assert_eq!(first[1].appearance_id, second[1].appearance_id);
        assert!(tracker.events.is_empty());
    }
}
