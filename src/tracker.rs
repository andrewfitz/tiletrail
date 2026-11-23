mod evidence;

use crate::markers::hard_cut;
use crate::vision::{Detection, Tile, appearance_distance};
use crate::*;
use evidence::{interior_weight, match_avatars};
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
                let base = score(tile, candidate, col < active_count);
                weights[(row, col)] = if col < active_count && base < 720 {
                    interior_weight(row, col, &tiles, &self.active, base)
                } else {
                    base
                };
            }
        }
        let avatar_matches = match_avatars(&tiles, &self.active);
        for &(row, column, weight) in &avatar_matches {
            weights[(row, column)] = weight;
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

        review_merged_slots(&tiles, &candidates, active_count, &mut decisions);
        review_placeholders(
            &tiles,
            &candidates,
            active_count,
            &avatar_matches,
            &mut decisions,
        );
        reconcile_stage_switches(&tiles, &candidates, active_count, &mut decisions);

        let mut next = Vec::with_capacity(tiles.len());
        let mut regions = Vec::with_capacity(tiles.len());
        let mut ambiguous_stages = Vec::new();
        for (row, decision) in decisions.iter().enumerate() {
            let matched = decision.matched;
            let tile = &tiles[row];
            let candidate = matched.map(|index| candidates[index]);
            let feed = if candidate.is_none() && tile.kind == TileKind::Video {
                feed_partner(row, &tiles)
            } else {
                FeedMatch::None
            };
            let duplicate = match feed {
                FeedMatch::Unique(peer) => {
                    simultaneous_identity(peer, &decisions, &candidates, &next)
                }
                _ => None,
            };
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
            let unclear_stage =
                matches!(feed, FeedMatch::Ambiguous) && tile.role == TileRole::Primary;
            if unclear_stage {
                ambiguous_stages.push(row);
            }
            let uncertain = unclear_stage
                || candidate.is_some_and(|sample| sample.uncertain)
                || (duplicate.is_none() && decision.ambiguous)
                || tile.kind == TileKind::Unknown;
            let mut possible_matches = decision.possible.clone();
            if let Some(candidate) = candidate.filter(|sample| sample.uncertain) {
                for alternative in &candidate.alternatives {
                    // Inherit only identities that can still be reviewed by
                    // the bounded tracker after a long, ambiguous sequence.
                    let id = alternative.participant_id;
                    if (self.dormant.contains_key(&id)
                        || self
                            .active
                            .iter()
                            .any(|sample| sample.participant == Some(id)))
                        && !possible_matches
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

        for stage in ambiguous_stages {
            for (peer, tile) in tiles.iter().enumerate() {
                if !similar_cross_role(&tiles[stage], tile) || next[peer].uncertain {
                    continue;
                }
                let Some(id) = next[peer].participant else {
                    continue;
                };
                if !regions[stage]
                    .possible_matches
                    .iter()
                    .any(|option| option.participant_id == id)
                {
                    let option = IdentityMatch {
                        participant_id: id,
                        confidence: REVIEW_MATCH as f32 / 1000.0,
                    };
                    next[stage].alternatives.push(option.clone());
                    regions[stage].possible_matches.push(option);
                }
            }
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

/// A disappearing gutter can make a detector report adjacent slots as one
/// video. A genuine reflow can also enlarge a tile, so preserve strong visual
/// continuity and review boxes that span several old slots without it.
fn review_merged_slots(
    tiles: &[Tile],
    candidates: &[&Sample],
    active_count: usize,
    decisions: &mut [Decision],
) {
    if active_count < 2 {
        return;
    }
    for (tile, decision) in tiles.iter().zip(decisions) {
        if tile.kind != TileKind::Video {
            continue;
        }
        let area = tile.rect.width * tile.rect.height;
        if let Some(prior) = decision.matched.map(|i| candidates[i]) {
            if prior.tile.rect.width * prior.tile.rect.height > 0.75 * area
                || (prior.tile.kind == TileKind::Video
                    && appearance_distance(&prior.tile.descriptor, &tile.descriptor)
                        <= 2.0 * FEED_DISTANCE)
            {
                continue;
            }
        }
        let mut occupied = 0.0;
        let mut occupants = Vec::new();
        for prior in &candidates[..active_count] {
            let Some(id) = prior.participant else {
                continue;
            };
            let rect = prior.tile.rect;
            let fraction = rect.width * rect.height / area;
            // A fully contained old slot has IoU equal to its area fraction.
            if (0.15..=0.75).contains(&fraction) && tile.rect.overlap(rect) >= 0.85 * fraction {
                occupied += fraction;
                occupants.push(id);
            }
        }
        if occupied < 0.8 || occupants.len() < 2 {
            continue;
        }
        decision.matched = None;
        decision.ambiguous = true;
        for id in occupants {
            if !decision
                .possible
                .iter()
                .any(|option| option.participant_id == id)
            {
                decision.possible.push(IdentityMatch {
                    participant_id: id,
                    confidence: REVIEW_MATCH as f32 / 1000.0,
                });
            }
        }
    }
}

/// A camera-off card provides no visual link to a missing video. Across a
/// reflow, the previously unseen card could represent any absent participant.
/// Exclude feeds that were actually recognized elsewhere in the frame.
fn review_placeholders(
    tiles: &[Tile],
    candidates: &[&Sample],
    active_count: usize,
    avatar_matches: &[(usize, usize, i32)],
    decisions: &mut [Decision],
) {
    if !tiles.iter().any(|tile| tile.kind == TileKind::CameraOff) {
        return;
    }
    let mut visible = std::collections::BTreeSet::new();
    let mut reflow = false;
    for (row, (tile, decision)) in tiles.iter().zip(decisions.iter()).enumerate() {
        let avatar = decision
            .matched
            .is_some_and(|col| avatar_matches.iter().any(|&(r, c, _)| r == row && c == col));
        if tile.kind != TileKind::Video && !avatar {
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

    for (row, (tile, decision)) in tiles.iter().zip(decisions).enumerate() {
        if tile.kind != TileKind::CameraOff
            || decision.matched.is_some_and(|col| {
                !reflow || avatar_matches.iter().any(|&(r, c, _)| r == row && c == col)
            })
        {
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
    // An unresolved card has only position to offer when its video returns.
    // Preserve provisional continuity, but let independently recognized video win.
    if tile.kind == TileKind::Video && prior.kind == TileKind::CameraOff && candidate.uncertain {
        return if active && spatial >= 0.9 {
            UNMATCHED + 1
        } else {
            0
        };
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
    index: usize,
    decisions: &[Decision],
    candidates: &[&Sample],
    assigned: &[Sample],
) -> Option<ParticipantId> {
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

/// If a stage switches to the exact picture of a continuously observed
/// thumbnail, the old stage occupant must not win on slot overlap alone.
fn reconcile_stage_switches(
    tiles: &[Tile],
    candidates: &[&Sample],
    active_count: usize,
    decisions: &mut [Decision],
) {
    if !tiles.iter().any(|tile| tile.role == TileRole::Thumbnail) {
        return;
    }
    for (row, stage) in tiles.iter().enumerate() {
        if stage.kind != TileKind::Video || stage.role != TileRole::Primary {
            continue;
        }
        let Some(previous) = decisions[row].matched.map(|i| candidates[i]) else {
            continue;
        };
        if appearance_distance(&previous.tile.descriptor, &stage.descriptor) <= 2.0 * FEED_DISTANCE
        {
            continue;
        }
        let FeedMatch::Unique(peer) = feed_partner(row, tiles) else {
            continue;
        };
        let Some(index) = decisions[peer].matched.filter(|&i| i < active_count) else {
            continue;
        };
        let anchor = candidates[index];
        if !anchor.uncertain
            && anchor.tile.kind == TileKind::Video
            && anchor.tile.role == TileRole::Thumbnail
            && anchor.tile.rect.overlap(tiles[peer].rect) >= 0.9
            && anchor.participant != previous.participant
            && appearance_distance(&anchor.tile.descriptor, &tiles[peer].descriptor) < FEED_DISTANCE
        {
            // The stage gets a new appearance on the thumbnail's track.
            decisions[row].matched = None;
        }
    }
}

const FEED_DISTANCE: f32 = 0.035;

fn similar_cross_role(tile: &Tile, other: &Tile) -> bool {
    tile.kind == TileKind::Video
        && other.kind == TileKind::Video
        && other.role != tile.role
        && other.rect.overlap(tile.rect) < 0.1
        && appearance_distance(&tile.descriptor, &other.descriptor) < FEED_DISTANCE
}

fn spatial_contrast(tile: &Tile) -> f32 {
    let Some(grid) = tile.descriptor.get(..48) else {
        return 0.0;
    };
    let mut mean = [0.0; 3];
    for pixel in grid.chunks_exact(3) {
        for (channel, &value) in pixel.iter().enumerate() {
            mean[channel] += value / 16.0;
        }
    }
    (grid
        .iter()
        .enumerate()
        .map(|(i, value)| (value - mean[i % 3]).powi(2))
        .sum::<f32>()
        / 48.0)
        .sqrt()
}

enum FeedMatch {
    None,
    Unique(usize),
    Ambiguous,
}

fn feed_partner(row: usize, tiles: &[Tile]) -> FeedMatch {
    let tile = &tiles[row];
    let mut similar = tiles
        .iter()
        .enumerate()
        .filter(|(_, other)| similar_cross_role(tile, other));
    let Some((peer, other)) = similar.next() else {
        return FeedMatch::None;
    };
    // A flat color gives no spatial evidence of a duplicated camera feed.
    if similar.next().is_some()
        || spatial_contrast(tile).min(spatial_contrast(other)) < FEED_DISTANCE
        || tiles
            .iter()
            .filter(|tile| similar_cross_role(other, tile))
            .count()
            != 1
    {
        return FeedMatch::Ambiguous;
    }
    FeedMatch::Unique(peer)
}

#[cfg(test)]
mod tests {
    use super::evidence::matching_interior;
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
        let mut stage = tile(0.0, 0.0, 0.65, 0.2);
        stage.descriptor[12..24].fill(0.75);
        let mut thumbnail = tile(0.75, 0.6, 0.25, 0.2);
        thumbnail.role = TileRole::Thumbnail;
        thumbnail.descriptor.clone_from(&stage.descriptor);
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

    #[test]
    fn unidentifiable_camera_cards_remain_provisional_until_the_video_returns() {
        let mut tracker = Tracker::default();
        let alice = tile(0.0, 0.0, 0.4, 0.2);
        let bob = tile(0.5, 0.0, 0.4, 0.8);
        let mut left = tile(0.0, 0.5, 0.4, 0.5);
        left.kind = TileKind::CameraOff;
        let mut right = left.clone();
        right.rect.x = 0.5;
        push(&mut tracker, 0, vec![alice.clone(), bob.clone()]);
        push(&mut tracker, 1, vec![left.clone(), right.clone()]);
        push(&mut tracker, 2, vec![left, right]);
        for frame in &tracker.observations[1..] {
            for card in &frame.regions {
                assert!(card.identity_uncertain);
                for id in [1, 2] {
                    assert!(
                        card.possible_matches
                            .iter()
                            .any(|option| option.participant_id == id)
                    );
                }
            }
        }
        assert!(tracker.events.iter().all(|event| event.uncertain));
        let mut alice = alice;
        let mut bob = bob;
        alice.rect.y = 0.5;
        bob.rect.y = 0.5;
        push(&mut tracker, 3, vec![alice, bob]);
        for (index, region) in tracker.observations[3].regions.iter().enumerate() {
            assert_eq!(region.participant_id, Some(index as u64 + 1));
            assert!(!region.identity_uncertain);
        }
    }

    #[test]
    fn flat_cross_role_feeds_have_no_confirmed_shared_identity() {
        let mut tracker = Tracker::default();
        let stage = tile(0.0, 0.0, 0.65, 0.2);
        let mut thumbnail = tile(0.75, 0.6, 0.25, 0.2);
        thumbnail.role = TileRole::Thumbnail;
        push(&mut tracker, 0, vec![stage, thumbnail]);
        let regions = &tracker.observations[0].regions;
        assert_ne!(regions[0].participant_id, regions[1].participant_id);
        assert!(regions[0].identity_uncertain);
        assert_eq!(
            regions[0].possible_matches[0].participant_id,
            regions[1].participant_id.unwrap()
        );
    }

    #[test]
    fn stage_switch_to_a_similar_feed_uses_the_continuing_thumbnail() {
        let mut tracker = Tracker::default();
        let view = |offset: f32, role: TileRole| {
            let mut tile = if role == TileRole::Primary {
                tile(0.0, 0.0, 0.65, 0.0)
            } else {
                tile(0.75, 0.6, 0.25, 0.0)
            };
            tile.role = role;
            tile.descriptor[..48].fill(offset);
            tile.descriptor[12..24].fill(offset + 0.3);
            tile
        };
        push(
            &mut tracker,
            0,
            vec![
                view(0.22, TileRole::Primary),
                view(0.35, TileRole::Thumbnail),
            ],
        );
        push(
            &mut tracker,
            1,
            vec![
                view(0.31, TileRole::Primary),
                view(0.34, TileRole::Thumbnail),
            ],
        );
        let old = &tracker.observations[0].regions;
        let now = &tracker.observations[1].regions;
        assert_ne!(old[0].participant_id, old[1].participant_id);
        assert_eq!(now[0].participant_id, old[1].participant_id);
        assert_ne!(now[0].appearance_id, old[0].appearance_id);
        assert_eq!(now[1].appearance_id, old[1].appearance_id);
    }
    #[test]
    fn a_weak_match_to_two_merged_slots_never_confirms_a_resize_or_move() {
        let mut tracker = Tracker::default();
        let left = tile(0.0, 0.0, 0.4, 0.2);
        let right = tile(0.4, 0.0, 0.4, 0.8);
        let mut merged = tile(0.0, 0.0, 0.4, 0.26);
        merged.rect.width = 0.8;
        push(&mut tracker, 0, vec![left.clone(), right.clone()]);
        push(&mut tracker, 1, vec![merged]);
        let region = &tracker.observations[1].regions[0];
        assert!(region.identity_uncertain);
        assert_eq!(region.possible_matches.len(), 2);
        assert!(tracker.events.iter().all(|event| event.uncertain));
        push(&mut tracker, 2, vec![left, right]);
        for (index, region) in tracker.observations[2].regions.iter().enumerate() {
            assert_eq!(region.participant_id, Some(index as u64 + 1));
            assert!(!region.identity_uncertain);
        }
    }
    #[test]
    fn distinctive_avatars_move_with_their_tracks_but_duplicate_cards_stay_provisional() {
        let avatar = |x, y, rgb: [f32; 3]| {
            let mut card = tile(x, y, 0.4, 0.1);
            card.kind = TileKind::CameraOff;
            for cell in [5, 6, 9, 10] {
                card.descriptor[3 * cell..3 * cell + 3].copy_from_slice(&rgb);
            }
            card.descriptor[48..].fill(0.0);
            for (c, value) in rgb.into_iter().enumerate() {
                card.descriptor[48 + c * 8] = 0.75;
                card.descriptor[48 + c * 8 + (value * 8.0) as usize] += 0.25;
            }
            card
        };
        let coral = [0.85, 0.3, 0.18];
        let blue = [0.1, 0.35, 0.8];
        for second in [blue, coral] {
            let mut tracker = Tracker::default();
            push(
                &mut tracker,
                0,
                vec![avatar(0.0, 0.0, coral), avatar(0.5, 0.0, second)],
            );
            push(
                &mut tracker,
                1,
                vec![avatar(0.0, 0.5, second), avatar(0.5, 0.5, coral)],
            );
            let regions = &tracker.observations[1].regions;
            if second == blue {
                assert_eq!(regions[0].participant_id, Some(2));
                assert_eq!(regions[1].participant_id, Some(1));
                assert!(regions.iter().all(|r| !r.identity_uncertain));
            } else {
                assert!(
                    regions
                        .iter()
                        .all(|r| r.identity_uncertain && r.possible_matches.len() >= 2)
                );
            }
        }
    }

    #[test]
    fn thumbnail_subdivision_requires_current_pixel_evidence_from_multiple_prior_crops() {
        let image = |inverted: bool| {
            let mut pixels = Vec::new();
            for y in 0..120 {
                for x in 0..160 {
                    let light = if (x / 20 + y / 20) % 2 == 0 { 0 } else { 65 };
                    let rgb = [60 + light, 30 + light, (y / 40) * 36 + light];
                    pixels.push(if inverted { rgb.map(|v| 255 - v) } else { rgb });
                }
            }
            vision::Measurements {
                width: 160,
                height: 120,
                pixels,
                rotation: Rotation::R0,
            }
        };
        let crop = |i: usize, count: usize, image: &vision::Measurements| {
            let mut t = tile(0.75, i as f32 / count as f32, 0.25, 0.0);
            t.rect.height = 1.0 / count as f32;
            t.role = TileRole::Thumbnail;
            vision::reobserve(image, &t).unwrap()
        };
        let before = image(false);
        let mut tracker = Tracker::default();
        push(
            &mut tracker,
            0,
            (0..3).map(|i| crop(i, 3, &before)).collect(),
        );
        for changed in [false, true] {
            let current = image(changed);
            let mut proposed = Detection {
                tiles: (0..5).map(|i| crop(i, 5, &current)).collect(),
                confidence: 1.0,
            };
            tracker.verify_thumbnail_subdivision(&current, &mut proposed);
            assert_eq!(proposed.tiles.len(), if changed { 5 } else { 3 });
        }
    }

    #[test]
    fn a_flat_center_or_single_unchanged_cell_cannot_anchor_a_redecorated_video() {
        let old = vec![0.2; 72];
        assert!(!matching_interior(&old, &old));
        let mut textured = old.clone();
        textured[15..18].fill(0.7);
        let mut changed = vec![0.95; 72];
        changed[15..18].fill(0.7);
        assert!(!matching_interior(&textured, &changed));
        for cell in [1, 4, 6] {
            changed[cell * 3..cell * 3 + 3].fill(0.2);
        }
        assert!(matching_interior(&textured, &changed));
    }
    #[test]
    fn a_shared_textured_background_cannot_confirm_replacement_occupants() {
        let view = |x, background: f32| {
            let mut view = tile(x, 0.0, 0.4, background);
            for (cell, color) in [(1, 0.15), (4, 0.2), (5, 0.7), (6, 0.3)] {
                view.descriptor[cell * 3..cell * 3 + 3].fill(color);
            }
            view
        };
        let mut tracker = Tracker::default();
        push(&mut tracker, 0, vec![view(0.0, 0.05), view(0.5, 0.15)]);
        push(&mut tracker, 1, vec![view(0.0, 0.95), view(0.5, 0.85)]);
        assert!(
            tracker.observations[1]
                .regions
                .iter()
                .all(|r| r.identity_uncertain)
        );
    }
    #[test]
    fn repeated_striped_thumbnail_crops_do_not_veto_a_new_subdivision() {
        let image = vision::Measurements {
            width: 160,
            height: 120,
            rotation: Rotation::R0,
            pixels: (0..160 * 120)
                .map(|i| {
                    if i % 160 % 32 < 16 {
                        [40, 65, 35]
                    } else {
                        [175, 110, 85]
                    }
                })
                .collect(),
        };
        let crop = |i, n| {
            let mut t = tile(0.6, i as f32 / n as f32, 0.4, 0.);
            t.rect.height = 1.0 / n as f32;
            t.role = TileRole::Thumbnail;
            vision::reobserve(&image, &t).unwrap()
        };
        let mut tracker = Tracker::default();
        push(&mut tracker, 0, (0..2).map(|i| crop(i, 2)).collect());
        let mut detection = Detection {
            tiles: (0..3).map(|i| crop(i, 3)).collect(),
            confidence: 1.,
        };
        tracker.verify_thumbnail_subdivision(&image, &mut detection);
        assert_eq!(detection.tiles.len(), 3);
    }
}
