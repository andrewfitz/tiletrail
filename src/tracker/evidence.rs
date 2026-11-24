use super::*;

impl Tracker {
    /// If a detector proposes more slots inside the same strip, test whether
    /// it cut through pictures that are still present at their previous bounds.
    pub(crate) fn verify_thumbnail_subdivision(
        &self,
        image: &vision::Measurements,
        detection: &mut Detection,
    ) {
        if self.active.is_empty() || detection.confidence < MIN_LAYOUT_CONFIDENCE {
            return;
        }
        let old_count = self
            .active
            .iter()
            .filter(|s| s.tile.role == TileRole::Thumbnail)
            .count();
        if old_count < 2
            || detection
                .tiles
                .iter()
                .filter(|t| t.role == TileRole::Thumbnail)
                .count()
                <= old_count
        {
            return;
        }
        let old: Vec<_> = self
            .active
            .iter()
            .filter(|s| s.tile.role == TileRole::Thumbnail)
            .map(|s| &s.tile)
            .collect();
        let proposed: Vec<_> = detection
            .tiles
            .iter()
            .filter(|t| t.role == TileRole::Thumbnail)
            .collect();
        let bounds = envelope(&old);
        let current = envelope(&proposed);
        let vertical = old.iter().all(|t| t.rect.width >= 0.95 * bounds.width)
            && proposed
                .iter()
                .all(|t| t.rect.width >= 0.95 * current.width);
        let horizontal = old.iter().all(|t| t.rect.height >= 0.95 * bounds.height)
            && proposed
                .iter()
                .all(|t| t.rect.height >= 0.95 * current.height);
        if (!vertical && !horizontal)
            || bounds.overlap(current) < 0.95
            || old.iter().any(|tile| tile.kind != TileKind::Video)
            || old
                .iter()
                .any(|tile| proposed.iter().any(|t| t.rect.overlap(tile.rect) >= 0.80))
        {
            return;
        }
        let Some(measured): Option<Vec<_>> = old
            .iter()
            .map(|tile| vision::reobserve(image, tile))
            .collect()
        else {
            return;
        };
        let mut verified = 0;
        for (i, (prior, now)) in old.iter().zip(&measured).enumerate() {
            let agrees = |a: &Tile, b: &Tile| {
                a.kind == b.kind
                    && (appearance_distance(&a.descriptor, &b.descriptor) <= 0.06
                        || matching_interior(&a.descriptor, &b.descriptor))
            };
            if spatial_contrast(prior) >= 0.025
                && agrees(prior, now)
                && !old
                    .iter()
                    .enumerate()
                    .any(|(j, other)| i != j && agrees(other, now))
                && !measured
                    .iter()
                    .enumerate()
                    .any(|(j, other)| i != j && agrees(prior, other))
            {
                verified += 1;
            }
        }
        // The fixed outer bounds and at least two independently verified slots
        // leave at most one changing feed. Its identity is still scored normally.
        if verified < 2 || verified + 1 < old_count {
            return;
        }
        detection
            .tiles
            .retain(|tile| tile.role != TileRole::Thumbnail);
        detection.tiles.extend(measured);
        detection.tiles.sort_by(|a, b| {
            a.rect
                .y
                .total_cmp(&b.rect.y)
                .then_with(|| a.rect.x.total_cmp(&b.rect.x))
        });
    }
}

/// A tile earns protection once the detector has returned it this many pushes
/// in a row, and keeps it for at most this many seconds of source time
/// without the detector returning it again.
const ESTABLISHED: u32 = 8;
const HOLD_SECONDS: i128 = 3;

/// How long the detector has returned an appearance's tile, and since when
/// the tracker has held it against the detector.
#[derive(Clone, Copy, Default)]
pub(crate) struct Steadiness {
    detected: u32,
    held_since: Option<Timestamp>,
}

/// The hold that began at `since` may still cover a frame at `now`.
fn within_hold(since: Timestamp, now: Timestamp) -> bool {
    since.common_clock(now).is_some_and(|(start, end)| {
        end - start
            <= HOLD_SECONDS
                * i128::from(since.time_base.denominator)
                * i128::from(now.time_base.denominator)
    })
}

impl Tracker {
    /// Hold established video tiles through detections that briefly cut one
    /// of them into pieces or merge several into one, while the pictures in
    /// their old bounds are still there.
    ///
    /// A dark band or a straight edge in someone's room can read as a gutter
    /// for a few frames and follows whatever moves; a flat moment can hide
    /// the gutters between tiles. A real change of layout replaces the
    /// pictures, so the old bounds no longer match what they showed. Only a
    /// tile the detector returned for several pushes in a row is held, and
    /// only for a few seconds, so a tile that was wrong to begin with cannot
    /// keep itself alive.
    pub(crate) fn hold_established_tiles(
        &mut self,
        image: &vision::Measurements,
        detection: &mut Detection,
        timestamp: Timestamp,
    ) {
        if self.active.is_empty() || detection.confidence < MIN_LAYOUT_CONFIDENCE {
            self.steady.clear();
            return;
        }
        let steady = &self.steady;
        let established = |sample: &Sample| {
            sample.tile.kind == TileKind::Video
                && spatial_contrast(&sample.tile) >= 0.025
                && steady.get(&sample.appearance).is_some_and(|state| {
                    state.detected >= ESTABLISHED
                        && state
                            .held_since
                            .is_none_or(|since| within_hold(since, timestamp))
                })
        };
        let unchanged = |prior: &Tile| {
            vision::reobserve(image, prior).filter(|now| {
                now.kind == prior.kind
                    && (appearance_distance(&prior.descriptor, &now.descriptor) <= 0.06
                        || matching_interior(&prior.descriptor, &now.descriptor))
            })
        };
        let mut cut = vec![false; detection.tiles.len()];
        let mut held = vec![false; self.active.len()];
        let mut restored = Vec::new();

        // One established tile cut into pieces that together fill its bounds.
        for (index, sample) in self.active.iter().enumerate() {
            let prior = &sample.tile;
            if !established(sample) {
                continue;
            }
            let pieces: Vec<usize> = (0..detection.tiles.len())
                .filter(|&piece| !cut[piece] && within(detection.tiles[piece].rect, prior.rect))
                .collect();
            let tiles: Vec<&Tile> = pieces
                .iter()
                .map(|&piece| &detection.tiles[piece])
                .collect();
            if tiles.len() < 2
                || envelope(&tiles).overlap(prior.rect) < 0.95
                || tiles
                    .iter()
                    .any(|tile| tile.rect.overlap(prior.rect) >= 0.80)
            {
                continue;
            }
            let Some(now) = unchanged(prior) else {
                continue;
            };
            for piece in pieces {
                cut[piece] = true;
            }
            held[index] = true;
            restored.push(now);
        }

        // Several established tiles merged into one that spans them. Each of
        // them must still show its own picture.
        for (merged, tile) in detection.tiles.iter().enumerate() {
            if cut[merged] {
                continue;
            }
            let span = tile.rect;
            let members: Vec<usize> = (0..self.active.len())
                .filter(|&index| !held[index] && within(self.active[index].tile.rect, span))
                .collect();
            let tiles: Vec<&Tile> = members
                .iter()
                .map(|&index| &self.active[index].tile)
                .collect();
            if tiles.len() < 2
                || envelope(&tiles).overlap(span) < 0.95
                || tiles.iter().any(|tile| tile.rect.overlap(span) >= 0.80)
                || !members
                    .iter()
                    .all(|&index| established(&self.active[index]))
            {
                continue;
            }
            let Some(now): Option<Vec<Tile>> = tiles.iter().map(|tile| unchanged(tile)).collect()
            else {
                continue;
            };
            cut[merged] = true;
            for index in members {
                held[index] = true;
            }
            restored.extend(now);
        }

        // Count what the detector itself returned before changing its answer.
        let mut steady = BTreeMap::new();
        for (index, sample) in self.active.iter().enumerate() {
            let mut state = self
                .steady
                .get(&sample.appearance)
                .copied()
                .unwrap_or_default();
            if held[index] {
                state.held_since.get_or_insert(timestamp);
            } else if detection
                .tiles
                .iter()
                .any(|tile| tile.rect.overlap(sample.tile.rect) >= 0.80)
            {
                state.detected += 1;
                state.held_since = None;
            } else {
                continue;
            }
            steady.insert(sample.appearance, state);
        }
        self.steady = steady;

        if restored.is_empty() {
            return;
        }
        let mut index = 0;
        detection.tiles.retain(|_| {
            index += 1;
            !cut[index - 1]
        });
        detection.tiles.extend(restored);
        detection.tiles.sort_by(|a, b| {
            a.rect
                .y
                .total_cmp(&b.rect.y)
                .then_with(|| a.rect.x.total_cmp(&b.rect.x))
        });
    }
}

/// The share of a rectangle that lies inside another is nearly all of it.
fn within(inner: Rect, outer: Rect) -> bool {
    let width =
        ((inner.x + inner.width).min(outer.x + outer.width) - inner.x.max(outer.x)).max(0.0);
    let height =
        ((inner.y + inner.height).min(outer.y + outer.height) - inner.y.max(outer.y)).max(0.0);
    let area = inner.width * inner.height;
    area > 0.0 && width * height >= 0.95 * area
}

fn envelope(tiles: &[&Tile]) -> Rect {
    let mut left = 1.0f32;
    let mut top = 1.0f32;
    let mut right = 0.0f32;
    let mut bottom = 0.0f32;
    for tile in tiles {
        left = left.min(tile.rect.x);
        top = top.min(tile.rect.y);
        right = right.max(tile.rect.x + tile.rect.width);
        bottom = bottom.max(tile.rect.y + tile.rect.height);
    }
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

// Subtract the channel-common intensity distribution. Neutral UI chrome,
// nameplates and matte then contribute little to an avatar's color signature.
// Normalization allows the same avatar to occupy a different fraction of a card.
struct Avatar {
    palette: [f32; 24],
    center: [f32; 2],
}

fn avatar_palette(tile: &Tile) -> Option<Avatar> {
    if tile.kind != TileKind::CameraOff || spatial_contrast(tile) < 0.025 {
        return None;
    }
    let histogram = tile.descriptor.get(48..72)?;
    let mut palette = [0.0; 24];
    let mut mass = 0.0;
    for bin in 0..8 {
        let neutral = histogram[bin]
            .min(histogram[8 + bin])
            .min(histogram[16 + bin]);
        for index in [bin, 8 + bin, 16 + bin] {
            palette[index] = histogram[index] - neutral;
            mass += palette[index];
        }
    }
    if mass < 0.06 {
        return None;
    }
    for value in &mut palette {
        *value /= mass;
    }
    let mut center = [0.0; 2];
    let mut chroma = 0.0;
    for (i, rgb) in tile.descriptor[..48].chunks_exact(3).enumerate() {
        let weight = rgb[0].max(rgb[1]).max(rgb[2]) - rgb[0].min(rgb[1]).min(rgb[2]);
        chroma += weight;
        center[0] += weight * (i % 4) as f32;
        center[1] += weight * (i / 4) as f32;
    }
    if chroma < 0.02 {
        return None;
    }
    center = center.map(|value| value / (4.0 * chroma));
    Some(Avatar { palette, center })
}

#[derive(Clone, Copy)]
struct Nearest {
    index: usize,
    distance: f32,
    second: f32,
}

impl Nearest {
    const EMPTY: Self = Self {
        index: usize::MAX,
        distance: f32::INFINITY,
        second: f32::INFINITY,
    };

    fn add(&mut self, index: usize, distance: f32) {
        if distance < self.distance {
            self.second = self.distance;
            self.distance = distance;
            self.index = index;
        } else {
            self.second = self.second.min(distance);
        }
    }

    fn unique(self) -> bool {
        self.distance < 0.30 && self.second - self.distance >= 0.12
    }
}

/// Promote only mutually distinctive, continuously observed avatar images.
/// Identical themed cards stay under the ordinary placeholder uncertainty rules.
pub(super) fn match_avatars(tiles: &[Tile], active: &[Sample]) -> Vec<(usize, usize, i32)> {
    let mut pairs = Vec::new();
    if !tiles.iter().any(|t| t.kind == TileKind::CameraOff) {
        return pairs;
    }
    let previous: Vec<_> = active
        .iter()
        .enumerate()
        .filter_map(|(i, s)| avatar_palette(&s.tile).map(|palette| (i, palette)))
        .collect();
    if previous.is_empty() {
        return pairs;
    }
    let mut columns = vec![Nearest::EMPTY; active.len()];
    let mut rows = vec![Nearest::EMPTY; tiles.len()];
    for (row, tile) in tiles.iter().enumerate() {
        let Some(current) = avatar_palette(tile) else {
            continue;
        };
        for (col, prior) in &previous {
            let color = current
                .palette
                .iter()
                .zip(&prior.palette)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                * 0.5;
            let spatial = appearance_distance(&tile.descriptor, &active[*col].tile.descriptor);
            let displacement =
                (current.center[0] - prior.center[0]).hypot(current.center[1] - prior.center[1]);
            // The centroid of chromatic structure must stay within half a grid
            // cell; palette equality alone cannot identify an unrelated image.
            let distance = if displacement <= 0.125 && spatial <= 0.20 {
                color + 0.4 * spatial
            } else {
                1.0
            };
            rows[row].add(*col, distance);
            columns[*col].add(row, distance);
        }
    }
    for (row, best) in rows.into_iter().enumerate() {
        if best.unique() && columns[best.index].unique() && columns[best.index].index == row {
            pairs.push((
                row,
                best.index,
                (1000.0 * (0.98 - 0.65 * best.distance)) as i32,
            ));
        }
    }
    pairs
}

/// A partial match can recover a decorated video's existing slot. If other
/// feeds contain that patch, retain the prior occupant only for review.
pub(super) fn interior_weight(
    row: usize,
    column: usize,
    tiles: &[Tile],
    active: &[Sample],
    base: i32,
) -> i32 {
    let tile = &tiles[row];
    let prior = &active[column].tile;
    if tile.kind != TileKind::Video
        || prior.kind != tile.kind
        || tile.rect.overlap(prior.rect) < 0.9
        || !matching_interior(&tile.descriptor, &prior.descriptor)
    {
        return base;
    }
    let shared = active.iter().enumerate().any(|(i, other)| {
        i != column
            && other.tile.kind == TileKind::Video
            && matching_interior(&tile.descriptor, &other.tile.descriptor)
    }) || tiles.iter().enumerate().any(|(i, other)| {
        i != row
            && other.kind == TileKind::Video
            && matching_interior(&prior.descriptor, &other.descriptor)
    });
    if shared { base.max(UNMATCHED) } else { 720 }
}

/// Decorations can replace most of a feed while a textured interior patch
/// remains fixed. Require two adjacent, distinguishable central samples as
/// well as supporting cells elsewhere in the image.
pub(super) fn matching_interior(left: &[f32], right: &[f32]) -> bool {
    let (Some(a), Some(b)) = (left.get(..48), right.get(..48)) else {
        return false;
    };
    let mut gaps = [0.0; 16];
    for (i, (old, new)) in a.chunks_exact(3).zip(b.chunks_exact(3)).enumerate() {
        gaps[i] = old.iter().zip(new).map(|(u, v)| (u - v).abs()).sum::<f32>() / 3.0;
    }
    if gaps.iter().filter(|&&gap| gap <= 0.065).count() < 4 {
        return false;
    }
    [(5, 6), (5, 9), (6, 10), (9, 10)]
        .into_iter()
        .any(|(u, v)| {
            gaps[u] <= 0.025
                && gaps[v] <= 0.025
                && (0..3)
                    .map(|c| (a[3 * u + c] - a[3 * v + c]).abs())
                    .sum::<f32>()
                    / 3.0
                    >= 0.05
        })
}
