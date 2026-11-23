//! Confirm a borderless layout using perpendicular image discontinuities. A
//! narrow sample band accommodates soft or slightly tilted captured seams.
use super::{Bounds, Measurements, color_gap};

fn cut(image: &Measurements, bounds: Bounds, horizontal: bool) -> Option<(usize, usize)> {
    let (start, end, lo, hi) = bounds.dimensions(horizontal);
    let extent = end - start;
    if extent < 20 || hi - lo < 16 {
        return None;
    }
    let score = |line| {
        let mut votes = 0;
        for i in 0..32 {
            let c = lo + (2 * i + 1) * (hi - lo) / 64;
            let a = image.at_line(horizontal, line - 2, c);
            let b = image.at_line(horizontal, line + 1, c);
            let gap = color_gap(a, b);
            votes += usize::from(gap >= 14);
            if gap >= 12 {
                let surround = color_gap(a, image.at_line(horizontal, line - 4, c))
                    .max(color_gap(b, image.at_line(horizontal, line + 3, c)));
                votes += usize::from(gap.saturating_sub(surround) >= 12);
            }
        }
        votes
    };
    (start + extent / 4..end - extent / 4)
        .map(|line| (line, score(line)))
        .max_by_key(|&(_, votes)| votes)
}

pub(super) fn find(image: &Measurements, bounds: Bounds) -> Option<Vec<Bounds>> {
    let (horizontal, (line, score)) = [true, false]
        .into_iter()
        .filter_map(|axis| Some((axis, cut(image, bounds, axis)?)))
        .max_by_key(|(_, (_, votes))| *votes)?;
    if score < 50 {
        return None;
    }
    let (start, end, _, _) = bounds.dimensions(horizontal);
    let a = bounds.with_extent(horizontal, start, line);
    let b = bounds.with_extent(horizontal, line, end);
    let mut cells = Vec::new();
    let mut ends = Vec::new();
    for side in [a, b] {
        let (start, end, _, _) = side.dimensions(!horizontal);
        if let Some((line, votes)) = cut(image, side, !horizontal).filter(|(_, votes)| *votes >= 44)
        {
            ends.push((line, votes));
            cells.push(side.with_extent(!horizontal, start, line));
            cells.push(side.with_extent(!horizontal, line, end));
        } else {
            cells.push(side);
        }
    }
    match ends.as_slice() {
        [(_, votes)] if *votes >= 54 => Some(cells),
        [(p, _), (q, _)] if p.abs_diff(*q) <= 4 => Some(cells),
        _ => None,
    }
}
