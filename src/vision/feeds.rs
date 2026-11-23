//! Detect ordinary composite layouts from gutters and coherent image edges.

use super::{
    Bounds, Detection, MAX_TILES, Measurements, Split, best_split, color_gap, feed_insets,
    feed_tile, frame_bounds, matte_color, matte_line, sort_tiles, trim_matte,
};

// Find a flat separator that crosses the image, including a colored one.
fn feed_matte(image: &Measurements, frame: Bounds) -> Option<[u8; 3]> {
    // The center pixel lets us check the candidate against the whole line
    // before walking a run of separator-colored lines.
    for horizontal in [true, false] {
        let (start, end, lo, hi) = frame.dimensions(horizontal);
        if end - start < 12 || hi - lo < 8 {
            continue;
        }
        let center = lo + (hi - lo) / 2;
        let mut line = start + 3;
        while line < end - 3 {
            let color = image.at_line(horizontal, line, center);
            if color_gap(color, image.at_line(horizontal, line - 1, center)) <= 14
                || !matte_line(image, color, horizontal, line, lo, hi)
            {
                line += 1;
                continue;
            }
            let mut after = line + 1;
            while after < end - 3 && matte_line(image, color, horizontal, after, lo, hi) {
                after += 1;
            }
            let occupied = |boundary| {
                (lo..hi)
                    .filter(|&i| color_gap(image.at_line(horizontal, boundary, i), color) > 14)
                    .count()
            };
            if after - line <= (end - start) / 12
                && occupied(line - 1).min(occupied(after)) * 3 >= hi - lo
            {
                return Some(color);
            }
            line = after;
        }
    }
    matte_color(image, frame)
}

fn feed_edge(
    image: &Measurements,
    bounds: Bounds,
    horizontal: bool,
    aligned_with: Option<usize>,
) -> Option<Split> {
    let (start, end, lo, hi) = bounds.dimensions(horizontal);
    let margin = ((end - start) / 12).max(5);
    if end - start < 2 * margin || hi - lo < 12 {
        return None;
    }
    let (first, last) = aligned_with.map_or((start + margin, end - margin), |line| {
        (
            (line.saturating_sub(3)).max(start + margin),
            (line + 3).min(end - margin),
        )
    });
    let mut best = None;
    let mut best_score = 0.0;
    for line in first..=last {
        let mut strong = 0;
        let mut salient = 0;
        let mut wide_salient = 0;
        let probes = (hi - lo).min(64);
        for probe in 0..probes {
            let i = lo + (2 * probe + 1) * (hi - lo) / (2 * probes);
            let at = |line| image.at_line(horizontal, line, i);
            let a = at(line - 1);
            let b = at(line);
            let gap = color_gap(a, b);
            let nearby = color_gap(at(line - 2), a).max(color_gap(b, at(line + 1)));
            strong += usize::from(gap >= 14);
            salient += usize::from(gap.saturating_sub(nearby) >= 8);
            if aligned_with.is_some() {
                let a = at(line - 2);
                let b = at(line + 1);
                let nearby = color_gap(at(line - 4), a).max(color_gap(b, at(line + 3)));
                wide_salient += usize::from(color_gap(a, b).saturating_sub(nearby) >= 12);
            }
        }
        let strength = strong as f32 / probes as f32;
        let salience = salient as f32 / probes as f32;
        let wide_salience = wide_salient as f32 / probes as f32;
        let accepted = if aligned_with.is_some() {
            strength >= 0.82 && (salience >= 0.40 || wide_salience >= 0.62)
        } else {
            strength >= 0.88 && salience >= 0.74
        };
        if accepted {
            let score = (salience.max(wide_salience) * 0.65 + strength * 0.35).min(0.98);
            if score > best_score {
                let boundary = aligned_with.unwrap_or(line);
                best_score = score;
                best = Some(Split {
                    first: bounds.with_extent(horizontal, start, boundary),
                    second: bounds.with_extent(horizontal, boundary, end),
                    horizontal,
                    score,
                });
            }
        }
    }
    best
}

fn feed_partition(
    image: &Measurements,
    bounds: Bounds,
    matte: Option<[u8; 3]>,
    nested: bool,
) -> Option<Split> {
    if matte.is_some() {
        if let Some(gutter) = best_split(image, bounds, matte, true) {
            return Some(gutter);
        }
    }
    [true, false]
        .into_iter()
        .filter_map(|horizontal| feed_edge(image, bounds, horizontal, None))
        // In smaller regions, furniture and other objects can span a large
        // fraction of the crop. Require a stronger unsupported boundary.
        .filter(|split| !nested || split.score >= 0.90)
        .max_by(|a, b| a.score.total_cmp(&b.score))
}

fn aligned_feed_partition(
    image: &Measurements,
    bounds: Bounds,
    parent: Split,
    sibling: Option<Split>,
) -> Option<Split> {
    let sibling = sibling.filter(|split| split.horizontal != parent.horizontal)?;
    let (_, first_end, _, _) = sibling.first.dimensions(sibling.horizontal);
    let (second_start, _, _, _) = sibling.second.dimensions(sibling.horizontal);
    let line = (first_end + second_start) / 2;
    // A seam supported in one half of a frame can corroborate a softer
    // continuation in the other half. Require local evidence on both sides.
    feed_edge(image, bounds, sibling.horizontal, Some(line))
}

pub(crate) fn detect_feeds(image: &Measurements) -> Detection {
    let frame = frame_bounds(image);
    let matte = feed_matte(image, frame);
    let trim = |bounds| matte.map_or(bounds, |color| trim_matte(image, bounds, color));
    let frame = trim(frame);
    let mut pending = vec![(frame, 1.0_f32, feed_partition(image, frame, matte, false))];
    let mut regions = Vec::new();
    while let Some((bounds, confidence, split)) = pending.pop() {
        if bounds.area() == 0 {
            continue;
        }
        if let Some(split) = split.filter(|_| pending.len() + regions.len() + 1 < MAX_TILES) {
            let first = trim(split.first);
            let second = trim(split.second);
            let first_split = feed_partition(image, first, matte, true);
            let second_split = feed_partition(image, second, matte, true);
            let confidence = confidence.min(split.score);
            pending.push((
                second,
                confidence,
                second_split.or_else(|| aligned_feed_partition(image, second, split, first_split)),
            ));
            pending.push((
                first,
                confidence,
                first_split.or_else(|| aligned_feed_partition(image, first, split, second_split)),
            ));
        } else {
            regions.push((bounds, confidence));
        }
    }
    let mut insets = Vec::new();
    for &(base, _) in &regions {
        let available = MAX_TILES - regions.len() - insets.len();
        if available == 0 {
            break;
        }
        insets.extend(
            feed_insets::find(image, base)
                .into_iter()
                .take(available)
                .map(|inset| (inset, 0.85)),
        );
    }
    regions.extend(insets);
    let mut tiles: Vec<_> = regions
        .into_iter()
        .map(|(bounds, confidence)| {
            feed_tile(image, bounds, bounds.display_rect(image), confidence)
        })
        .collect();
    sort_tiles(&mut tiles);
    let confidence = tiles
        .iter()
        .map(|tile| tile.confidence)
        .reduce(f32::min)
        .unwrap_or(0.0);
    Detection { tiles, confidence }
}
