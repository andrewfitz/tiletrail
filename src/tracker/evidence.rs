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
