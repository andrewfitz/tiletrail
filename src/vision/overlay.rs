//! Identify a filmstrip pinned to a corner of a shared or mostly empty canvas.
//! Its tile boundaries end at the filmstrip and do not cross the main image.

use super::{Bounds, Measurements, color_gap, trim_matte};

pub(super) struct Overlay {
    pub tiles: Vec<Bounds>,
    band: Bounds,
    horizontal: bool,
}

impl Overlay {
    pub fn stage(&self, image: &Measurements, frame: Bounds, matte: Option<[u8; 3]>) -> Bounds {
        let axis = self.horizontal;
        let (start, end, cross_start, cross_end) = frame.dimensions(axis);
        let (left, right, top, bottom) = self.band.dimensions(axis);
        let main = if start == left {
            frame.with_extent(axis, right, end)
        } else {
            frame.with_extent(axis, start, left)
        };
        let stage = matte.map_or(main, |color| trim_matte(image, main, color));
        // If content continues beyond the shorter filmstrip, the filmstrip
        // overlays the stage. Its unoccluded side supplies the letterbox bounds.
        let outside = (cross_start..top).chain(bottom..cross_end);
        let visible = outside.step_by(3).any(|i| {
            let pixel = image.at_line(axis, (left + right) / 2, i);
            matte.is_none_or(|m| color_gap(pixel, m) > 12)
        });
        if visible {
            stage.with_extent(axis, start, end)
        } else {
            stage
        }
    }
}

const MIN_EDGE_SCORE: i32 = 18;

struct Edge {
    band: Bounds,
    score: i32,
}

// Fit the longest supported prefix from this corner. The strip-facing pixel
// must differ from a quiet, neutral background on the opposite side.
fn find_edge(
    image: &Measurements,
    frame: Bounds,
    horizontal: bool,
    trailing: bool,
    reverse: bool,
) -> Option<Edge> {
    let (_, extent, _, length) = frame.dimensions(horizontal);
    let position = |k| if reverse { length - 1 - k } else { k };
    let mut score = 0;
    let mut best = None;
    for width in 7..=extent / 3 {
        let (inside, outside, adjacent) = if trailing {
            (extent - width, extent - width - 1, extent - width - 3)
        } else {
            (width - 1, width, width + 2)
        };
        let mut strength = 0;
        for k in (0..length).step_by(2) {
            // Even perfect support adds only four points per remaining probe.
            // Check in batches to avoid spending a branch on every pixel.
            if k % 16 == 0
                && strength + (length - k).div_ceil(2) as i32 * 4 <= score.max(MIN_EDGE_SCORE - 1)
            {
                break;
            }
            let i = position(k);
            let pixel = image.at_line(horizontal, outside, i);
            let high = *pixel.iter().max().unwrap();
            let low = *pixel.iter().min().unwrap();
            let neutral = high - low <= 24 && (high < 60 || low > 185);
            let supported = neutral
                && color_gap(pixel, image.at_line(horizontal, adjacent, i)) <= 12
                && color_gap(pixel, image.at_line(horizontal, inside, i)) >= 22;
            strength += if supported { 4 } else { -6 };
            if k >= width / 2 && strength > score {
                score = strength;
                best = Some((width, k + 1));
            }
        }
    }
    let (width, mut span) = best?;
    if score < MIN_EDGE_SCORE || span < 16 {
        return None;
    }
    let (left, right) = if trailing {
        (extent - width, extent)
    } else {
        (0, width)
    };
    let exterior = if trailing { left - 1 } else { right };
    let mut quiet_run = 0;
    let supported_span = span;
    for k in supported_span..length {
        let i = position(k);
        let backdrop = image.at_line(horizontal, exterior, i);
        // A pillarbox can hide the edge of the terminal thumbnail.
        let filled = (left..right)
            .step_by(2)
            .filter(|&j| color_gap(backdrop, image.at_line(horizontal, j, i)) > 24)
            .count();
        if filled * 8 >= width {
            span = k + 1;
            quiet_run = 0;
        } else {
            quiet_run += 1;
            if quiet_run >= 3 {
                break;
            }
        }
    }
    let top = if reverse { length - span } else { 0 };
    Some(Edge {
        band: frame
            .with_extent(horizontal, left, right)
            .with_extent(!horizontal, top, top + span),
        score,
    })
}

fn subdivisions(image: &Measurements, edge: Edge, horizontal: bool) -> Option<(i32, Overlay)> {
    let (left, right, top, bottom) = edge.band.dimensions(horizontal);
    let width = right - left;
    let span = bottom - top;
    let mut seams = vec![0; span];
    for (k, votes) in seams.iter_mut().enumerate().skip(1) {
        let line = top + k;
        for i in left..right {
            let gap = color_gap(
                image.at_line(!horizontal, line - 1, i),
                image.at_line(!horizontal, line, i),
            );
            *votes += i32::from(gap >= 16) + i32::from(gap >= 40);
        }
    }
    let mut best = None;
    for count in 2..=(span / 8).min(12) {
        if span * 4 < count * width || span > count * width * 3 {
            continue;
        }
        let mut tiles = Vec::with_capacity(count);
        let mut previous = top;
        let mut score = edge.score;
        for j in 1..count {
            let center = (span * j + count / 2) / count;
            let tolerance = (span / count / 10).clamp(1, 3);
            let (k, &votes) = seams
                .iter()
                .enumerate()
                .take(center + tolerance + 1)
                .skip(center - tolerance)
                .max_by_key(|(k, votes)| (**votes, std::cmp::Reverse(k.abs_diff(center))))
                .unwrap();
            if votes * 100 < 66 * width as i32 {
                break;
            }
            score += votes * 100 / width as i32 - 60;
            tiles.push(edge.band.with_extent(!horizontal, previous, top + k));
            previous = top + k;
        }
        if tiles.len() + 1 == count && best.as_ref().is_none_or(|(old, _)| score > *old) {
            tiles.push(edge.band.with_extent(!horizontal, previous, bottom));
            best = Some((
                score,
                Overlay {
                    tiles,
                    band: edge.band,
                    horizontal,
                },
            ));
        }
    }
    best
}

pub(super) fn find(image: &Measurements, frame: Bounds) -> Option<Overlay> {
    let mut best = None;
    for horizontal in [false, true] {
        for trailing in [false, true] {
            for reverse in [false, true] {
                if let Some(candidate) = find_edge(image, frame, horizontal, trailing, reverse)
                    .and_then(|edge| subdivisions(image, edge, horizontal))
                {
                    if best.as_ref().is_none_or(|(score, _)| candidate.0 > *score) {
                        best = Some(candidate);
                    }
                }
            }
        }
    }
    best.map(|(_, strip)| strip)
}
