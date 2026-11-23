//! Fully framed insets whose trim separates two textured image regions.

use super::{Bounds, MAX_TILES, Measurements, color_gap};

const MIN_INSIDE: usize = 16;
// A chroma-interpolated edge can mute a full row of otherwise uniform trim.
const MAX_TRIM_GAP: u8 = 26;
const MIN_EDGE: u8 = 32;
const COLOR_BIN_WIDTH: usize = 8;

pub(super) fn find(image: &Measurements, frame: Bounds) -> Vec<Bounds> {
    let mut insets = Vec::new();
    if frame.right - frame.left < MIN_INSIDE + 4 || frame.bottom - frame.top < MIN_INSIDE + 4 {
        return insets;
    }

    for y in frame.top + 1..frame.bottom - MIN_INSIDE - 2 {
        for x in frame.left + 1..frame.right - MIN_INSIDE - 2 {
            let trim = image.at(x, y);
            if color_gap(trim, image.at(x - 1, y)) < MIN_EDGE
                || color_gap(trim, image.at(x, y - 1)) < MIN_EDGE
                || color_gap(trim, image.at(x + 1, y)) > MAX_TRIM_GAP
                || color_gap(trim, image.at(x, y + 1)) > MAX_TRIM_GAP
            {
                continue;
            }

            // Exposed upper-left corners fix both runs. Each must end before
            // the frame edge and meet an independently flat opposite side.
            let right =
                (x + 2..frame.right).find(|&i| color_gap(image.at(i, y), trim) > MAX_TRIM_GAP);
            let bottom =
                (y + 2..frame.bottom).find(|&i| color_gap(image.at(x, i), trim) > MAX_TRIM_GAP);
            let (Some(right), Some(bottom)) = (right, bottom) else {
                continue;
            };
            let outside = Bounds {
                left: x,
                top: y,
                right,
                bottom,
            };
            if right - x < MIN_INSIDE + 2
                || bottom - y < MIN_INSIDE + 2
                || outside.area() * 2 > frame.area()
                || insets.iter().any(|&inset| overlaps(inset, outside))
            {
                continue;
            }

            if let Some(interior) = interior(image, outside, trim)
                .filter(|&inner| distinct_footage(image, frame, outside, inner))
            {
                insets.push(interior);
                if insets.len() == MAX_TILES {
                    return insets;
                }
            }
        }
    }
    insets
}

fn overlaps(a: Bounds, b: Bounds) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn interior(image: &Measurements, bounds: Bounds, trim: [u8; 3]) -> Option<Bounds> {
    let mut inner = bounds;
    let max_trim = ((bounds.right - bounds.left).min(bounds.bottom - bounds.top) / 8).max(1);
    let mut thickness = [0; 4];
    for (axis, horizontal) in [true, false].into_iter().enumerate() {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        // Chroma subsampling and compression can shift a few pixels where
        // the trim touches the source images.
        let flat = |line| {
            let length = cross_end - cross_start;
            let misses = (cross_start..cross_end)
                .filter(|&i| color_gap(image.at_line(horizontal, line, i), trim) > MAX_TRIM_GAP)
                .take((length / 10) + 1)
                .count();
            misses <= length / 10
        };
        let leading = (start..=start + max_trim).find(|&line| !flat(line))?;
        let trailing = (end - max_trim - 1..end).rev().find(|&line| !flat(line))? + 1;
        if leading == start || trailing == end || trailing - leading < MIN_INSIDE {
            return None;
        }
        thickness[2 * axis] = leading - start;
        thickness[2 * axis + 1] = end - trailing;
        inner = inner.with_extent(horizontal, leading, trailing);
    }
    // A nearly even trim is stronger evidence than four coincidental lines.
    if thickness.iter().max()? - thickness.iter().min()? > 2 {
        return None;
    }

    for horizontal in [true, false] {
        let (start, end, _, _) = bounds.dimensions(horizontal);
        let (leading, trailing, cross_start, cross_end) = inner.dimensions(horizontal);
        for line in [start - 1, leading, trailing - 1, end] {
            let separated = (cross_start..cross_end)
                .filter(|&i| color_gap(image.at_line(horizontal, line, i), trim) >= MIN_EDGE)
                .count();
            if separated * 100 < (cross_end - cross_start) * 86 {
                return None;
            }
        }
    }
    Some(inner)
}

#[derive(Default)]
struct ColorSamples {
    channels: [[usize; 256 / COLOR_BIN_WIDTH]; 3],
    count: usize,
}

impl ColorSamples {
    fn add(&mut self, pixel: [u8; 3]) {
        for (bins, channel) in self.channels.iter_mut().zip(pixel) {
            bins[usize::from(channel) / COLOR_BIN_WIDTH] += 1;
        }
        self.count += 1;
    }

    fn textured(&self) -> bool {
        // Leave room for occasional highlights, text, or a small logo.
        self.channels.iter().any(|bins| {
            let mut cumulative = 0;
            let lower = bins.iter().position(|n| {
                cumulative += n;
                cumulative * 5 >= self.count
            });
            let mut cumulative = 0;
            let upper = bins.iter().position(|n| {
                cumulative += n;
                cumulative * 5 >= self.count * 4
            });
            upper.zip(lower).is_some_and(|(high, low)| high >= low + 2)
        })
    }

    fn distance(&self, other: &Self) -> usize {
        // Compare cumulative histograms so neighboring shades contribute less
        // than large color shifts. Results are in approximate channel levels.
        let total = self.count * other.count;
        self.channels
            .iter()
            .zip(&other.channels)
            .map(|(a, b)| {
                let mut sum_a = 0;
                let mut sum_b = 0;
                let mut distance = 0;
                for (n, m) in a.iter().zip(b) {
                    sum_a += n;
                    sum_b += m;
                    distance += (sum_a * other.count).abs_diff(sum_b * self.count);
                }
                distance * COLOR_BIN_WIDTH / total
            })
            .sum()
    }
}

fn distinct_footage(image: &Measurements, frame: Bounds, outer: Bounds, inner: Bounds) -> bool {
    let width = inner.right - inner.left;
    let height = inner.bottom - inner.top;
    let mut inside = ColorSamples::default();
    let mut outside = ColorSamples::default();
    for y in 0..12 {
        for x in 0..12 {
            inside.add(image.at(
                inner.left + (2 * x + 1) * width / 24,
                inner.top + (2 * y + 1) * height / 24,
            ));
        }
    }
    for horizontal in [true, false] {
        let (frame_start, frame_end, _, _) = frame.dimensions(horizontal);
        let (start, end, left, right) = outer.dimensions(horizontal);
        for (edge, margin, leading) in [
            (start, start - frame_start, true),
            (end, frame_end - end, false),
        ] {
            let depth = margin.min(width.min(height) / 2);
            let layers = depth.min(12);
            for i in 0..layers {
                let layer = (2 * i + 1) * depth / (2 * layers);
                let line = if leading {
                    edge - 1 - layer
                } else {
                    edge + layer
                };
                for i in 0..12 {
                    let cross = left + (2 * i + 1) * (right - left) / 24;
                    outside.add(image.at_line(horizontal, line, cross));
                }
            }
        }
    }
    inside.textured() && outside.textured() && inside.distance(&outside) >= 62
}
