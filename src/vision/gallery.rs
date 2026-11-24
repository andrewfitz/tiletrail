//! Recover a dense rectangular gallery when browser chrome has also been split
//! into apparent tiles. Repeated matte gutters must bound populated cells in
//! both directions. This cannot infer positions on a partially empty last row.
use super::{Bounds, MAX_TILES, Measurements, color_gap};

fn spans(
    image: &Measurements,
    frame: Bounds,
    horizontal: bool,
    matte: [u8; 3],
) -> Vec<(usize, usize)> {
    let (start, end, lo, hi) = frame.dimensions(horizontal);
    let probes = 24.min(hi - lo);
    let mut spans = Vec::new();
    let mut first = start;
    for line in start..=end {
        let gutter = line == end
            || (0..probes)
                .filter(|&i| {
                    color_gap(
                        image.at_line(
                            horizontal,
                            line,
                            lo + (2 * i + 1) * (hi - lo) / (2 * probes),
                        ),
                        matte,
                    ) <= 8
                })
                .count()
                * 2
                >= probes;
        if gutter {
            if line - first >= 6 {
                spans.push((first, line));
            }
            first = line + 1;
        }
    }
    let mut best = 0..0;
    for i in 0..spans.len() {
        let size = spans[i].1 - spans[i].0;
        let mut j = i + 1;
        while j < spans.len()
            && (spans[j].1 - spans[j].0).abs_diff(size) <= 2.max(size / 10)
            && spans[j].0 - spans[j - 1].1 <= 2.max(size / 4)
        {
            j += 1;
        }
        if j - i > best.len() {
            best = i..j;
        }
    }
    spans[best].to_vec()
}

fn neutral_matte(image: &Measurements, frame: Bounds) -> [u8; 3] {
    let mut colors = [0usize; 12];
    for y in (frame.top..frame.bottom).step_by(4) {
        for x in (frame.left..frame.right).step_by(4) {
            let p = image.at(x, y);
            let high = *p.iter().max().unwrap();
            if high < 96 && high - *p.iter().min().unwrap() <= 10 {
                colors[usize::from(high) / 8] += 1;
            }
        }
    }
    let (bucket, _) = colors.iter().enumerate().max_by_key(|(_, v)| *v).unwrap();
    [(bucket * 8 + 3) as u8; 3]
}

fn populated(image: &Measurements, bounds: Bounds, matte: [u8; 3]) -> bool {
    let Bounds {
        left,
        right,
        top,
        bottom,
    } = bounds;
    (0..5)
        .flat_map(|y| (0..5).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            color_gap(
                image.at(
                    left + (2 * x + 1) * (right - left) / 10,
                    top + (2 * y + 1) * (bottom - top) / 10,
                ),
                matte,
            ) > 12
        })
        .count()
        >= 13
}

pub(super) fn hinted(
    image: &Measurements,
    frame: Bounds,
    columns: usize,
    rows: usize,
) -> Option<Vec<Bounds>> {
    if columns * rows < 2 {
        return None;
    }
    let locate = |matte| {
        let row_spans = spans(image, frame, true, matte);
        let column_spans = spans(image, frame, false, matte);
        if row_spans.len() != rows || column_spans.len() != columns {
            return None;
        }
        populated_cells(image, row_spans, &column_spans, matte)
    };
    let neutral = neutral_matte(image, frame);
    locate(neutral).or_else(|| {
        super::feeds::feed_matte(image, frame)
            .filter(|&matte| color_gap(neutral, matte) > 8)
            .and_then(locate)
    })
}

fn populated_cells(
    image: &Measurements,
    rows: Vec<(usize, usize)>,
    columns: &[(usize, usize)],
    matte: [u8; 3],
) -> Option<Vec<Bounds>> {
    let mut cells = Vec::with_capacity(columns.len() * rows.len());
    for (top, bottom) in rows {
        for &(left, right) in columns {
            let bounds = Bounds {
                left,
                right,
                top,
                bottom,
            };
            if !populated(image, bounds, matte) {
                return None;
            }
            cells.push(bounds);
        }
    }
    Some(cells)
}

pub(super) fn recover(
    image: &Measurements,
    frame: Bounds,
    regions: &[(Bounds, f32, bool)],
) -> Option<Vec<Bounds>> {
    if regions.len() < 8 {
        return None;
    }
    let matte = neutral_matte(image, frame);
    let rows = spans(image, frame, true, matte);
    if rows.len() < 2 {
        return None;
    }
    let columns = spans(image, frame, false, matte);
    let count = rows.len() * columns.len();
    if columns.len() < 2 || !(8..=MAX_TILES).contains(&count) {
        return None;
    }
    let gallery = Bounds {
        left: columns[0].0,
        right: columns.last()?.1,
        top: rows[0].0,
        bottom: rows.last()?.1,
    };
    // Recovery may remove browser chrome outside a regular lattice. A
    // separately recognized document is valid content beside the gallery.
    let mut has_chrome = false;
    for (r, _, document) in regions {
        let x = (r.left + r.right) / 2;
        let y = (r.top + r.bottom) / 2;
        if x < gallery.left || x >= gallery.right || y < gallery.top || y >= gallery.bottom {
            if *document {
                return None;
            }
            has_chrome = true;
        }
    }
    if !has_chrome {
        return None;
    }
    let cells = populated_cells(image, rows, &columns, matte)?;
    let supported = cells
        .iter()
        .filter(|bounds| {
            regions.iter().any(|(region, _, _)| {
                let intersection = region
                    .right
                    .min(bounds.right)
                    .saturating_sub(region.left.max(bounds.left))
                    * region
                        .bottom
                        .min(bounds.bottom)
                        .saturating_sub(region.top.max(bounds.top));
                3 * intersection >= bounds.area() + region.area()
            })
        })
        .count();
    (supported * 2 >= count).then_some(cells)
}

// A colored avatar tile often shades the same hue across a smooth backdrop.
// Verify the hue on both sides and require a distinct icon in the middle.
pub(super) fn colored_card(image: &Measurements, b: Bounds) -> bool {
    let (width, height) = (b.right - b.left, b.bottom - b.top);
    let at = |x, y| {
        image.at(
            b.left + (2 * x + 1) * width / 24,
            b.top + (2 * y + 1) * height / 24,
        )
    };
    let reference = at(2, 5);
    let high = *reference.iter().max().unwrap();
    if high < 24 || high - *reference.iter().min().unwrap() < high / 4 {
        return false;
    }
    let mut consistent = 0;
    let mut outside = 0;
    let mut icon = false;
    for y in 1..10 {
        for x in 1..11 {
            let p = at(x, y);
            if (4..8).contains(&x) && (4..8).contains(&y) {
                icon |= color_gap(p, reference) > 28
                    && color_gap(p, at(x - 1, y)).max(color_gap(p, at(x, y - 1))) >= 12;
                continue;
            }
            let peak = u32::from(*p.iter().max().unwrap());
            let same_hue = (0..3).all(|k| {
                (u32::from(p[k]) * u32::from(high)).abs_diff(u32::from(reference[k]) * peak) * 12
                    <= peak * u32::from(high)
            });
            let smooth = color_gap(
                p,
                image.at(
                    (b.left + (2 * x + 1) * width / 24 + 1).min(b.right - 1),
                    b.top + (2 * y + 1) * height / 24,
                ),
            ) <= 10;
            consistent += usize::from(peak >= 12 && same_hue && smooth);
            outside += 1;
        }
    }
    icon && consistent * 5 >= outside * 4
}
