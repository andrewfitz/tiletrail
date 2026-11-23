//! A shared document may leave room at one edge for a single inset participant.
//! The outer matte around that participant ends repeatedly at the document edge.

use super::{Bounds, Measurements, SAMPLE_LIMIT, color_gap, matte_color, surface, trim_matte};

pub(super) fn find(
    image: &Measurements,
    frame: Bounds,
    matte: [u8; 3],
) -> Option<(Bounds, Bounds)> {
    for horizontal in [false, true] {
        let (begin, end, cross_begin, cross_end) = frame.dimensions(horizontal);
        let extent = end - begin;
        let length = cross_end - cross_begin;
        if extent < 24 || length < 16 {
            continue;
        }
        for trailing in [false, true] {
            let at = |offset, cross| {
                let line = if trailing {
                    extent - 1 - offset
                } else {
                    offset
                };
                image.at_line(horizontal, begin + line, cross_begin + cross)
            };
            // A genuine sidebar has matte on both sides of the tile. Probe a
            // small fixed number of scanlines and vote for the end of that matte.
            let probes = 24.min(length);
            let mut votes = [0u8; SAMPLE_LIMIT as usize / 2 + 1];
            for i in 0..probes {
                let cross = (2 * i + 1) * length / (2 * probes);
                if color_gap(at(0, cross), matte) <= 9 {
                    if let Some(width) =
                        (1..extent / 2).find(|&k| color_gap(at(k, cross), matte) > 9)
                    {
                        votes[width] += 1;
                    }
                }
            }
            let (width, count) = votes[6..extent / 2]
                .iter()
                .enumerate()
                .max_by_key(|(_, v)| *v)?;
            if usize::from(*count) * 4 < probes {
                continue;
            }
            let width = width + 6;
            let (side, stage) = if trailing {
                (
                    frame.with_extent(horizontal, end - width, end),
                    frame.with_extent(horizontal, begin, end - width),
                )
            } else {
                (
                    frame.with_extent(horizontal, begin, begin + width),
                    frame.with_extent(horizontal, begin + width, end),
                )
            };
            let tile = trim_matte(image, side, matte);
            let (left, right, top, bottom) = tile.dimensions(horizontal);
            if bottom - top < 6 || (right - left) * 4 < width * 3 {
                continue;
            }
            let stage = trim_matte(image, stage, matte);
            if stage.area() == 0 {
                continue;
            }
            let stage = if surface(image, stage).document {
                stage
            } else {
                let Some(color) = matte_color(image, stage).filter(|c| *c != matte) else {
                    continue;
                };
                let trimmed = trim_matte(image, stage, color);
                if trimmed.area() == 0 || !surface(image, trimmed).document {
                    continue;
                }
                trimmed
            };
            if top >= cross_begin + 3 && bottom + 3 <= cross_end {
                return Some((stage, tile));
            }
            // A corner preview can extend from a letterbox over the document.
            // Follow its straight inner edge across the matte/document junction.
            let (inner, outer) = if top == cross_begin && bottom + 3 <= cross_end {
                (bottom - 1, bottom + 1)
            } else if bottom == cross_end && top >= cross_begin + 3 {
                (top, top - 2)
            } else {
                continue;
            };
            let mut extension = 0;
            let mut misses = 0;
            for offset in width..extent / 2 {
                let position = if trailing {
                    end - 1 - offset
                } else {
                    begin + offset
                };
                let a = image.at_line(horizontal, position, inner);
                let b = image.at_line(horizontal, position, outer);
                if color_gap(a, b) >= 18 {
                    extension = offset + 1 - width;
                    misses = 0;
                } else {
                    misses += 1;
                    if misses >= 3 {
                        break;
                    }
                }
            }
            if extension >= 4 && extension * 6 >= bottom - top {
                let tile = if trailing {
                    tile.with_extent(horizontal, end - width - extension, right)
                } else {
                    tile.with_extent(horizontal, left, begin + width + extension)
                };
                return Some((stage, tile));
            }
        }
    }
    None
}
