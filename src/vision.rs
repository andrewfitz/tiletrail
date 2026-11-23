#[path = "vision/cards.rs"]
mod cards;
#[path = "vision/feed_insets.rs"]
mod feed_insets;
#[path = "vision/feeds.rs"]
mod feeds;
#[path = "vision/gallery.rs"]
mod gallery;
#[path = "vision/junction.rs"]
mod junction;
#[path = "vision/overlay.rs"]
mod overlay;
#[path = "vision/sidebar.rs"]
mod sidebar;

use crate::{Frame, PixelFormat, Rect, Rotation, TileKind, TileRole};
pub(crate) use feeds::detect_feeds;

const SAMPLE_LIMIT: u32 = 256;
const MAX_TILES: usize = crate::MAX_GRID_FEEDS as usize;
const GRID_SIZE: usize = 4;
const GRID_VALUES: usize = GRID_SIZE * GRID_SIZE * 3;
const HISTOGRAM_BINS: usize = 8;
const DESCRIPTOR_VALUES: usize = GRID_VALUES + 3 * HISTOGRAM_BINS;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measurements {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
    pub rotation: Rotation,
}

pub(crate) fn sample_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let longest = u64::from(width.max(height));
    if longest <= u64::from(SAMPLE_LIMIT) {
        return (width, height);
    }
    let scale =
        |size| ((u64::from(size) * u64::from(SAMPLE_LIMIT) + longest / 2) / longest).max(1) as u32;
    (scale(width), scale(height))
}

pub(crate) fn measure(frame: &Frame<'_>) -> Measurements {
    let (width, height) = sample_dimensions(frame.width, frame.height);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    let bytes_per_pixel = frame.format.bytes_per_pixel();
    let sample_at = |i, source, target| {
        (((2 * u64::from(i) + 1) * u64::from(source)) / (2 * u64::from(target))) as usize
    };
    // Every row uses the same source columns. Compute the exact center
    // coordinates once, including when only a single sample fits an axis.
    let mut offsets = [0; SAMPLE_LIMIT as usize];
    let offsets = &mut offsets[..width as usize];
    for (x, offset) in offsets.iter_mut().enumerate() {
        *offset = sample_at(x as u32, frame.width, width) * bytes_per_pixel;
    }
    for y in 0..height {
        let row = &frame.data[sample_at(y, frame.height, height) * frame.stride..];
        match frame.format {
            PixelFormat::Gray8 => pixels.extend(offsets.iter().map(|&i| [row[i]; 3])),
            PixelFormat::Rgb24 | PixelFormat::Rgba8 => {
                pixels.extend(offsets.iter().map(|&i| [row[i], row[i + 1], row[i + 2]]));
            }
            PixelFormat::Bgra8 => {
                pixels.extend(offsets.iter().map(|&i| [row[i + 2], row[i + 1], row[i]]));
            }
        }
    }
    Measurements {
        width,
        height,
        pixels,
        rotation: frame.display.rotation,
    }
}

#[derive(Clone)]
pub(crate) struct Tile {
    pub rect: Rect,
    pub kind: TileKind,
    pub role: TileRole,
    pub descriptor: Vec<f32>,
    pub confidence: f32,
}

pub(crate) struct Detection {
    pub tiles: Vec<Tile>,
    pub confidence: f32,
}

#[derive(Clone, Copy)]
struct Bounds {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Bounds {
    fn area(self) -> usize {
        (self.right - self.left) * (self.bottom - self.top)
    }

    fn dimensions(self, horizontal: bool) -> (usize, usize, usize, usize) {
        if horizontal {
            (self.top, self.bottom, self.left, self.right)
        } else {
            (self.left, self.right, self.top, self.bottom)
        }
    }

    fn with_extent(mut self, horizontal: bool, start: usize, end: usize) -> Self {
        if horizontal {
            self.top = start;
            self.bottom = end;
        } else {
            self.left = start;
            self.right = end;
        }
        self
    }

    fn display_rect(self, image: &Measurements) -> Rect {
        let x = self.left as f32 / image.width as f32;
        let y = self.top as f32 / image.height as f32;
        let width = (self.right - self.left) as f32 / image.width as f32;
        let height = (self.bottom - self.top) as f32 / image.height as f32;
        let reverse_x = (image.width as usize - self.right) as f32 / image.width as f32;
        let reverse_y = (image.height as usize - self.bottom) as f32 / image.height as f32;
        let (x, y, width, height) = match image.rotation {
            Rotation::R0 => (x, y, width, height),
            Rotation::R90 => (reverse_y, x, height, width),
            Rotation::R180 => (reverse_x, reverse_y, width, height),
            Rotation::R270 => (y, reverse_x, height, width),
        };
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Measurements {
    fn at(&self, x: usize, y: usize) -> [u8; 3] {
        self.pixels[y * self.width as usize + x]
    }

    fn at_line(&self, horizontal: bool, line: usize, i: usize) -> [u8; 3] {
        if horizontal {
            self.at(i, line)
        } else {
            self.at(line, i)
        }
    }
}

fn color_gap(a: [u8; 3], b: [u8; 3]) -> u8 {
    a[0].abs_diff(b[0])
        .max(a[1].abs_diff(b[1]))
        .max(a[2].abs_diff(b[2]))
}

// Use a neutral dark frame-edge color as possible matte. Perimeter sampling leaves
// the backgrounds of colorful edge-to-edge feeds out of the matte model.
fn matte_color(image: &Measurements, bounds: Bounds) -> Option<[u8; 3]> {
    let mut buckets = [0_usize; 8];
    for horizontal in [false, true] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        for line in [start, end - 1] {
            for i in cross_start..cross_end {
                let pixel = image.at_line(horizontal, line, i);
                let high = *pixel.iter().max().unwrap();
                let low = *pixel.iter().min().unwrap();
                if high < 64 && high - low <= 8 {
                    let level =
                        (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3;
                    buckets[level as usize / 8] += 1;
                }
            }
        }
    }
    let dark_support = buckets[0];
    let (index, support) = buckets
        .into_iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)?;
    let perimeter = 2 * (bounds.right - bounds.left + bounds.bottom - bounds.top);
    if dark_support * 6 >= perimeter {
        return Some([3; 3]);
    }
    if support * 10 >= perimeter {
        return Some([(index * 8 + 3) as u8; 3]);
    }

    // A narrow black gutter can touch two opposite edges without making up much
    // of the perimeter. Require a full crossing before accepting black as matte.
    let black = [3; 3];
    for horizontal in [true, false] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        if end - start < 3 {
            continue;
        }
        let crosses_frame = (start + 1..end - 1).any(|line| {
            color_gap(image.at_line(horizontal, line, cross_start), black) <= 9
                && color_gap(image.at_line(horizontal, line, cross_end - 1), black) <= 9
                && matte_line(image, black, horizontal, line, cross_start, cross_end)
        });
        if crosses_frame {
            return Some(black);
        }
    }
    None
}

fn matte_line(
    image: &Measurements,
    matte: [u8; 3],
    horizontal: bool,
    line: usize,
    cross_start: usize,
    cross_end: usize,
) -> bool {
    let max_outside = (cross_end - cross_start) * 4 / 100;
    let mut outside = 0;
    for i in cross_start..cross_end {
        outside += usize::from(color_gap(image.at_line(horizontal, line, i), matte) > 9);
        if outside > max_outside {
            return false;
        }
    }
    true
}

fn trim_matte(image: &Measurements, mut bounds: Bounds, matte: [u8; 3]) -> Bounds {
    // A second pass picks up side bars once wide top or bottom bars are eliminated.
    for horizontal in [true, false, true, false] {
        let (mut start, mut end, cross_start, cross_end) = bounds.dimensions(horizontal);
        while start < end && matte_line(image, matte, horizontal, start, cross_start, cross_end) {
            start += 1;
        }
        while start < end && matte_line(image, matte, horizontal, end - 1, cross_start, cross_end) {
            end -= 1;
        }
        bounds = bounds.with_extent(horizontal, start, end);
        if bounds.area() == 0 {
            break;
        }
    }
    bounds
}

// Controls can cover the middle of a gallery gutter. Require the matte to
// continue across most of the line, including both outer thirds.
fn gutter_line(
    image: &Measurements,
    matte: [u8; 3],
    horizontal: bool,
    line: usize,
    start: usize,
    end: usize,
) -> bool {
    let length = end - start;
    let probes = [1, 3, 5, 7].map(|i| start + i * length / 8);
    if probes
        .iter()
        .all(|&i| color_gap(image.at_line(horizontal, line, i), matte) > 9)
    {
        return false;
    }
    let mut outside = 0;
    let mut ends = [0; 2];
    for i in start..end {
        if color_gap(image.at_line(horizontal, line, i), matte) > 9 {
            outside += 1;
            if outside * 3 > length {
                return false;
            }
        } else if i < start + length / 3 {
            ends[0] += 1;
        } else if i >= end - length / 3 {
            ends[1] += 1;
        }
    }
    ends.into_iter().all(|n| n * 7 >= length)
}

// Probe a fixed lattice so the cost of identifying large, nearly uniform
// presentation backgrounds and application panels is independent of area.
#[derive(Default)]
struct Surface {
    document: bool,
    sidebar: bool,
    dark_canvas: bool,
}

fn surface(image: &Measurements, bounds: Bounds) -> Surface {
    let w = bounds.right - bounds.left;
    let h = bounds.bottom - bounds.top;
    let mut whites = [0; 8];
    let mut dark = [0; 16];
    let mut ink = 0;
    let mut count = 0;
    for y in (bounds.top..bounds.bottom).step_by((h / 20).max(1)) {
        for x in (bounds.left..bounds.right).step_by((w / 24).max(1)) {
            let pixel = image.at(x, y);
            let high = *pixel.iter().max().unwrap();
            let low = *pixel.iter().min().unwrap();
            if low >= 192
                && high - low <= 24
                && color_gap(pixel, image.at((x + 1).min(bounds.right - 1), y)) <= 4
            {
                whites[usize::from(low - 192) / 8] += 1;
            }
            if high <= 63 && high - low <= 10 {
                dark[usize::from(high) / 4] += 1;
            }
            ink += usize::from(high < 170);
            count += 1;
        }
    }
    let white = whites
        .windows(2)
        .map(|pair| pair[0] + pair[1])
        .max()
        .unwrap_or(0);
    let dark_total: usize = dark.iter().sum();
    let dark_levels = dark.iter().filter(|&&n| n * 10 >= count).count();
    Surface {
        document: white * 100 >= count * 56 && ink * 100 >= count,
        dark_canvas: dark.iter().take(3).sum::<usize>() * 100 >= count * 56 && dark_total < count,
        sidebar: dark_total * 100 >= count * 88 && dark_levels >= 2,
    }
}

#[derive(Clone, Copy)]
struct Split {
    first: Bounds,
    second: Bounds,
    horizontal: bool,
    score: f32,
}

fn best_split(
    image: &Measurements,
    bounds: Bounds,
    matte: Option<[u8; 3]>,
    require_gutter: bool,
) -> Option<Split> {
    let mut best: Option<Split> = None;
    for horizontal in [true, false] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        let min_extent = (if horizontal {
            image.height
        } else {
            image.width
        } / 20)
            .clamp(4, 12) as usize;
        if end - start < 2 * min_extent || cross_end - cross_start < 4 {
            continue;
        }

        let mut line = start + min_extent;
        while line <= end - min_extent {
            let mut gutter_end = line;
            if let Some(matte) = matte {
                while gutter_end < end - min_extent
                    && gutter_line(image, matte, horizontal, gutter_end, cross_start, cross_end)
                {
                    gutter_end += 1;
                }
            }
            let score = if gutter_end > line {
                // Distinguish a gutter from a flat dark band inside a dark feed.
                let lit = |line| {
                    (cross_start..cross_end)
                        .filter(|&i| {
                            color_gap(image.at_line(horizontal, line, i), matte.unwrap()) > 14
                        })
                        .count()
                };
                let occupied =
                    lit(line - 1).min(lit(gutter_end)) as f32 / (cross_end - cross_start) as f32;
                // Centered rows can leave most of a seam adjacent to the matte.
                if occupied >= 0.30 {
                    0.65 + 0.35 * occupied
                } else {
                    0.0
                }
            } else if require_gutter {
                0.0
            } else {
                let mut support = 0;
                let mut salient = 0;
                let max_misses = (cross_end - cross_start) * 22 / 100;
                for i in cross_start..cross_end {
                    let a = image.at_line(horizontal, line - 1, i);
                    let b = image.at_line(horizontal, line, i);
                    let gap = color_gap(a, b);
                    if gap >= 14 {
                        support += 1;
                        let nearby = color_gap(a, image.at_line(horizontal, line - 2, i))
                            .max(color_gap(b, image.at_line(horizontal, line + 1, i)));
                        salient += usize::from(gap.saturating_sub(nearby) >= 8);
                    } else if i + 1 - cross_start - support > max_misses {
                        break;
                    }
                }
                let length = (cross_end - cross_start) as f32;
                if support as f32 / length < 0.78 {
                    0.0
                } else {
                    0.65 * salient as f32 / length + 0.35 * support as f32 / length
                }
            };
            let small_side = (line - start).min(end - gutter_end);
            let thin_band = small_side * 3 < cross_end - cross_start
                && (end - start) * 2 < (cross_end - cross_start) * 5
                && small_side * 3 < end - start;
            if score >= 0.72 && !(thin_band && line == gutter_end) {
                let balance = (line - start).min(end - gutter_end) as f32 / (end - start) as f32;
                let candidate = Split {
                    first: bounds.with_extent(horizontal, start, line),
                    second: bounds.with_extent(horizontal, gutter_end, end),
                    horizontal,
                    score: score * 0.87
                        + balance * 0.10
                        + if gutter_end > line { 0.10 } else { 0.0 },
                };
                if best.is_none_or(|existing| candidate.score > existing.score) {
                    best = Some(candidate);
                }
            }
            line = (line + 1).max(gutter_end);
        }
    }
    best
}

fn describe(image: &Measurements, bounds: Bounds) -> (Vec<f32>, TileKind, bool) {
    let stored_width = bounds.right - bounds.left;
    let stored_height = bounds.bottom - bounds.top;
    let (width, height) = match image.rotation {
        Rotation::R0 | Rotation::R180 => (stored_width, stored_height),
        Rotation::R90 | Rotation::R270 => (stored_height, stored_width),
    };
    let display_pixel = |x, y| {
        let (x, y) = match image.rotation {
            Rotation::R0 => (x, y),
            Rotation::R90 => (y, stored_height - 1 - x),
            Rotation::R180 => (stored_width - 1 - x, stored_height - 1 - y),
            Rotation::R270 => (stored_width - 1 - y, x),
        };
        image.at(bounds.left + x, bounds.top + y)
    };
    let mut descriptor = Vec::with_capacity(DESCRIPTOR_VALUES);
    let mut histogram = [[0_u32; HISTOGRAM_BINS]; 3];
    let mut dark_levels = [0_u32; 32];
    let mut neutral = 0;
    let mut bright_neutral = 0;
    for grid_y in 0..GRID_SIZE {
        let top = grid_y * height / GRID_SIZE;
        let bottom = (grid_y + 1) * height / GRID_SIZE;
        for grid_x in 0..GRID_SIZE {
            let left = grid_x * width / GRID_SIZE;
            let right = (grid_x + 1) * width / GRID_SIZE;
            // Tiny tiles have empty grid cells. Reuse nearby pixels for the
            // grid descriptor without counting them again in the histogram.
            let has_pixels = left < right && top < bottom;
            let right = right.max(left + 1);
            let bottom = bottom.max(top + 1);
            let mut sum = [0_u32; 3];
            for y in top..bottom {
                for x in left..right {
                    let pixel = display_pixel(x, y);
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixel[channel]);
                        if has_pixels {
                            histogram[channel][usize::from(pixel[channel]) / 32] += 1;
                        }
                    }
                    if has_pixels {
                        let max = *pixel.iter().max().unwrap();
                        let min = *pixel.iter().min().unwrap();
                        if max - min <= 8 && max < 128 {
                            dark_levels[usize::from(max) / 4] += 1;
                        }
                        neutral += usize::from(max - min <= 12);
                        bright_neutral += usize::from(min >= 180 && max - min <= 20);
                    }
                }
            }
            descriptor.extend(
                sum.map(|value| value as f32 / (255 * (bottom - top) * (right - left)) as f32),
            );
        }
    }
    let histogram_size = bounds.area() as u32;
    let flat = histogram.iter().all(|bins| {
        bins.windows(2)
            .any(|pair| (pair[0] + pair[1]) * 100 >= histogram_size * 88)
    });
    let solid_card = dark_levels.windows(2).enumerate().any(|(level, pair)| {
        if (pair[0] + pair[1]) * 100 < histogram_size * 48 {
            return false;
        }
        // Pillarboxed portrait video also has large matching dark side bars.
        // An avatar card shows its background above or below the central icon.
        let background = [(level * 4 + 4) as u8; 3];
        let vertical = (0..height)
            .step_by(2)
            .filter(|&y| color_gap(display_pixel(width / 2, y), background) <= 8)
            .count();
        let horizontal = (0..width)
            .step_by(2)
            .filter(|&x| color_gap(display_pixel(x, height / 2), background) <= 8)
            .count();
        vertical * 12 >= height && horizontal * 12 >= width
    });
    let kind = if solid_card
        || flat
            && neutral as f32 / histogram_size as f32 >= 0.85
            && bright_neutral as f32 / histogram_size as f32 <= 0.5
    {
        TileKind::CameraOff
    } else {
        TileKind::Video
    };
    let presentation_hint = bright_neutral as f32 / histogram_size as f32 >= 0.50;
    descriptor.extend(
        histogram
            .into_iter()
            .flatten()
            .map(|bin| bin as f32 / histogram_size as f32),
    );
    (descriptor, kind, presentation_hint)
}

/// Re-measure a previous display-space crop against the current sampled image.
pub(crate) fn reobserve(image: &Measurements, prior: &Tile) -> Option<Tile> {
    let Rect {
        x,
        y,
        width: w,
        height: h,
    } = prior.rect;
    let (x, y, w, h) = match image.rotation {
        Rotation::R0 => (x, y, w, h),
        Rotation::R90 => (y, 1.0 - x - w, h, w),
        Rotation::R180 => (1.0 - x - w, 1.0 - y - h, w, h),
        Rotation::R270 => (1.0 - y - h, x, h, w),
    };
    let at = |p: f32, limit: u32| (p.clamp(0.0, 1.0) * limit as f32).round() as usize;
    let bounds = Bounds {
        left: at(x, image.width),
        right: at(x + w, image.width),
        top: at(y, image.height),
        bottom: at(y + h, image.height),
    };
    if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
        return None;
    }
    let (descriptor, kind, _) = describe(image, bounds);
    Some(Tile {
        rect: bounds.display_rect(image),
        descriptor,
        kind,
        role: prior.role,
        confidence: prior.confidence,
    })
}

pub(crate) fn appearance_distance(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != DESCRIPTOR_VALUES || right.len() != DESCRIPTOR_VALUES {
        return 1.0;
    }
    let differences = left.iter().zip(right).map(|(a, b)| (a - b).abs());
    let squared_gap = differences
        .clone()
        .take(GRID_VALUES)
        .map(|d| d * d)
        .sum::<f32>();
    let grid_gap = (squared_gap / GRID_VALUES as f32).sqrt();
    let hist_gap = differences.skip(GRID_VALUES).sum::<f32>() / 6.0;
    (0.85 * grid_gap + 0.15 * hist_gap).clamp(0.0, 1.0)
}

fn frame_bounds(image: &Measurements) -> Bounds {
    Bounds {
        left: 0,
        top: 0,
        right: image.width as usize,
        bottom: image.height as usize,
    }
}

fn feed_tile(image: &Measurements, bounds: Bounds, rect: Rect, confidence: f32) -> Tile {
    Tile {
        rect,
        kind: TileKind::Video,
        role: TileRole::Primary,
        descriptor: describe(image, bounds).0,
        confidence,
    }
}

pub(crate) fn grid_feeds(image: &Measurements, columns: u32, rows: u32) -> Detection {
    // Transform each display cell back into coded coordinates to sample it.
    // Retain exact display geometry even when the sampling lattice is coarse.
    let columns = columns as usize;
    let rows = rows as usize;
    let width = image.width as usize;
    let height = image.height as usize;
    let (display_width, display_height) = match image.rotation {
        Rotation::R0 | Rotation::R180 => (width, height),
        Rotation::R90 | Rotation::R270 => (height, width),
    };
    let extent = |position: usize, count: usize, size: usize| {
        let start = position * size / count;
        let end = ((position + 1) * size / count).max(start + 1);
        (start, end)
    };
    let mut tiles = Vec::with_capacity(columns * rows);
    for row in 0..rows {
        for column in 0..columns {
            let (left, right) = extent(column, columns, display_width);
            let (top, bottom) = extent(row, rows, display_height);
            let (left, top, right, bottom) = match image.rotation {
                Rotation::R0 => (left, top, right, bottom),
                Rotation::R90 => (top, height - right, bottom, height - left),
                Rotation::R180 => (width - right, height - bottom, width - left, height - top),
                Rotation::R270 => (width - bottom, left, width - top, right),
            };
            let bounds = Bounds {
                left,
                top,
                right,
                bottom,
            };
            let rect = Rect {
                x: column as f32 / columns as f32,
                y: row as f32 / rows as f32,
                width: 1.0 / columns as f32,
                height: 1.0 / rows as f32,
            };
            tiles.push(feed_tile(image, bounds, rect, 1.0));
        }
    }
    Detection {
        tiles,
        confidence: 1.0,
    }
}

fn sort_tiles(tiles: &mut [Tile]) {
    tiles.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
}

pub(crate) fn detect(image: &Measurements) -> Detection {
    if image.width == 0 || image.height == 0 {
        return Detection {
            tiles: Vec::new(),
            confidence: 0.0,
        };
    }
    let whole_frame = frame_bounds(image);
    let matte = matte_color(image, whole_frame);
    let cards = matte.and_then(|_| cards::find(image, whole_frame));
    let framed_strip = cards.is_some();
    let (stage, cards) = cards.unwrap_or((whole_frame, Vec::new()));
    let mut regions: Vec<_> = cards.into_iter().map(|b| (b, 0.85, false)).collect();
    let mut side_panels = Vec::new();
    let canvas = surface(image, stage);
    let overlay = (!framed_strip && (canvas.document || canvas.dark_canvas))
        .then(|| overlay::find(image, whole_frame))
        .flatten();
    let has_overlay = overlay.is_some();
    let mut pending = Vec::new();
    if let Some(strip) = overlay {
        if canvas.document {
            let stage = strip.stage(image, whole_frame, matte);
            if stage.area() > 0 {
                regions.push((stage, 0.86, true));
            }
        }
        regions.extend(strip.tiles.into_iter().filter_map(|bounds| {
            let tile = matte.map_or(bounds, |color| trim_matte(image, bounds, color));
            (tile.area() > 0).then_some((tile, 0.85, false))
        }));
    } else if let Some((document, tile)) =
        matte.and_then(|color| sidebar::find(image, stage, color))
    {
        regions.push((document, 0.86, true));
        regions.push((tile, 0.85, false));
    } else {
        pending.push((stage, 1.0_f32));
    }
    while let Some((bounds, confidence)) = pending.pop() {
        let trimmed = matte.map_or(bounds, |color| trim_matte(image, bounds, color));
        if trimmed.area() == 0 {
            continue;
        }
        let surface = surface(image, trimmed);
        let side_panel = trimmed.bottom - trimmed.top >= whole_frame.bottom * 9 / 10
            && (trimmed.right == whole_frame.right || trimmed.left == 0)
            && (trimmed.right - trimmed.left) * 2 < whole_frame.right;
        if surface.sidebar && side_panel {
            side_panels.push((trimmed, confidence, false));
            continue;
        }
        if !surface.document && pending.len() + regions.len() + side_panels.len() + 1 < MAX_TILES {
            if let Some(split) = best_split(image, trimmed, matte, false) {
                let confidence = confidence.min(split.score);
                pending.push((split.second, confidence));
                pending.push((split.first, confidence));
                continue;
            }
        }
        regions.push((trimmed, confidence, surface.document));
    }

    // A tall, dark panel alone is ambiguous. Suppress it only beside a
    // two-dimensional gallery, so a dark portrait in a two-up call survives.
    let gallery = !side_panels.is_empty()
        && regions.len() >= 4
        && regions.iter().any(|(a, _, _)| {
            regions
                .iter()
                .any(|(b, _, _)| a.right <= b.left || b.right <= a.left)
                && regions
                    .iter()
                    .any(|(b, _, _)| a.bottom <= b.top || b.bottom <= a.top)
        });
    if !gallery {
        regions.extend(side_panels);
    }

    let recovered = gallery::recover(image, whole_frame, &regions);
    let in_grid = recovered.is_some();
    if let Some(cells) = recovered {
        regions = cells.into_iter().map(|b| (b, 0.9, false)).collect();
    }

    if let [(bounds, _, false)] = regions.as_slice() {
        if let Some(cells) = junction::find(image, *bounds) {
            regions = cells.into_iter().map(|b| (b, 0.75, false)).collect();
        }
    }

    let largest = regions
        .iter()
        .map(|(bounds, _, _)| bounds.area())
        .max()
        .unwrap_or(0);
    let stage_layout = regions.len() > 1
        && regions
            .iter()
            .any(|(bounds, _, _)| bounds.area() * 3 <= largest);
    let solo = regions.len() == 1;
    let mut tiles: Vec<_> = regions
        .into_iter()
        .map(|(bounds, confidence, document)| {
            let (descriptor, mut kind, presentation_hint) = describe(image, bounds);
            if in_grid && kind == TileKind::Video && gallery::colored_card(image, bounds) {
                kind = TileKind::CameraOff;
            }
            if document {
                kind = TileKind::ScreenShare;
            }
            let role = if has_overlay && !document || stage_layout && bounds.area() * 3 <= largest {
                TileRole::Thumbnail
            } else {
                if stage_layout && bounds.area() == largest && presentation_hint {
                    kind = TileKind::ScreenShare;
                }
                TileRole::Primary
            };
            // An unpartitioned image provides weak evidence that the source is a call.
            let confidence = if solo {
                0.55
            } else {
                confidence.clamp(0.0, 1.0)
            };
            Tile {
                rect: bounds.display_rect(image),
                kind,
                role,
                descriptor,
                confidence,
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplayGeometry, Timestamp};

    const RED: [u8; 3] = [190, 49, 33];
    const GREEN: [u8; 3] = [45, 163, 64];
    const BLUE: [u8; 3] = [36, 63, 176];
    const GRAY: [u8; 3] = [82; 3];

    fn scene(paint: &[(usize, usize, usize, usize, [u8; 3])]) -> Measurements {
        let mut image = Measurements {
            width: 160,
            height: 96,
            pixels: vec![[7; 3]; 160 * 96],
            rotation: Rotation::R0,
        };
        for &(left, top, width, height, color) in paint {
            for y in top..top + height {
                image.pixels[y * 160 + left..y * 160 + left + width].fill(color);
            }
        }
        image
    }

    fn assert_box(tile: &Tile, normalized: [f32; 4]) {
        for (actual, expected) in [tile.rect.x, tile.rect.y, tile.rect.width, tile.rect.height]
            .into_iter()
            .zip(normalized)
        {
            assert!(
                (actual - expected).abs() < 0.0001,
                "{:?} versus {normalized:?}",
                tile.rect
            );
        }
    }

    #[test]
    fn inset_camera_left_of_a_document_survives_the_matte() {
        let image = scene(&[
            (40, 4, 120, 88, [245; 3]),
            (50, 20, 80, 6, [70; 3]),
            (0, 36, 40, 25, GREEN),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 2);
        let thumbnail = result
            .tiles
            .iter()
            .find(|tile| tile.role == TileRole::Thumbnail)
            .unwrap();
        assert_box(thumbnail, [0.0, 36.0 / 96.0, 0.25, 25.0 / 96.0]);
        assert!(
            result
                .tiles
                .iter()
                .any(|tile| tile.kind == TileKind::ScreenShare)
        );
    }

    #[test]
    fn adjacent_cards_require_foreground_inside_each_surface() {
        let mut paint = vec![(0, 12, 160, 84, GREEN)];
        for (i, level) in [25, 35, 45].into_iter().enumerate() {
            paint.push((20 * i, 0, 20, 12, [level; 3]));
        }
        let frame = Bounds {
            left: 0,
            top: 0,
            right: 160,
            bottom: 96,
        };
        assert!(cards::find(&scene(&paint), frame).is_none());
        for i in 0..3 {
            paint.push((i * 20 + 8, 4, 4, 4, RED));
        }
        let (stage, tiles) = cards::find(&scene(&paint), frame).unwrap();
        assert_eq!(stage.top, 12);
        assert_eq!(tiles.len(), 3);
        assert_eq!(
            (tiles[2].left, tiles[2].right, tiles[2].bottom),
            (40, 60, 12)
        );
    }

    #[test]
    fn a_single_edge_does_not_establish_a_borderless_gallery() {
        let frame = Bounds {
            left: 0,
            top: 0,
            right: 160,
            bottom: 96,
        };
        let mut paint = vec![(0, 0, 160, 96, RED), (65, 0, 95, 96, BLUE)];
        assert!(junction::find(&scene(&paint), frame).is_none());
        paint.push((65, 52, 95, 44, GREEN));
        let cells = junction::find(&scene(&paint), frame).unwrap();
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn dense_gallery_recovery_keeps_a_separate_document() {
        let frame = Bounds {
            left: 0,
            top: 0,
            right: 160,
            bottom: 96,
        };
        let document = frame.with_extent(false, 0, 23);
        let mut regions = vec![(document, 0.86, true)];
        let mut paint = vec![(0, 0, 23, 96, [238; 3]), (1, 20, 21, 4, GRAY)];
        for row in 0..3 {
            for col in 0..3 {
                let left = 30 + 43 * col;
                let top = 2 + 32 * row;
                paint.push((left, top, 40, 28, RED));
                regions.push((
                    Bounds {
                        left,
                        top,
                        right: left + 40,
                        bottom: top + 28,
                    },
                    0.85,
                    false,
                ));
            }
        }
        let image = scene(&paint);
        assert!(gallery::recover(&image, frame, &regions).is_none());
        regions[0].2 = false;
        assert_eq!(gallery::recover(&image, frame, &regions).unwrap().len(), 9);
    }

    #[test]
    fn colored_cards_need_a_distinct_center() {
        let frame = Bounds {
            left: 0,
            top: 0,
            right: 160,
            bottom: 96,
        };
        let mut paint = vec![(0, 0, 160, 96, GREEN)];
        assert!(!gallery::colored_card(&scene(&paint), frame));
        paint.push((65, 40, 30, 16, [215; 3]));
        assert!(gallery::colored_card(&scene(&paint), frame));
    }

    #[test]
    fn padded_bgra_measurement_ignores_alpha_and_preserves_rotation() {
        let data = [
            30, 20, 10, 1, 60, 50, 40, 2, 255, 255, 90, 80, 70, 4, 120, 110, 100, 5,
        ];
        let frame = Frame {
            data: &data,
            width: 2,
            height: 2,
            stride: 10,
            format: PixelFormat::Bgra8,
            timestamp: Timestamp::new(0, 1, 30),
            display: DisplayGeometry {
                rotation: Rotation::R90,
                ..Default::default()
            },
        };
        let measured = measure(&frame);
        assert_eq!(
            measured.pixels,
            [[10, 20, 30], [40, 50, 60], [70, 80, 90], [100, 110, 120]]
        );
        assert_eq!(measured.rotation, Rotation::R90);
        assert_eq!(sample_dimensions(1920, 1080), (256, 144));
        assert_eq!(sample_dimensions(1, u32::MAX), (1, 256));
    }

    #[test]
    fn downsampling_reads_the_center_pixel_of_each_sampling_interval() {
        let data: Vec<_> = (0..512).map(|x| if x % 2 == 0 { 5 } else { 25 }).collect();
        let frame = Frame {
            data: &data,
            width: 512,
            height: 1,
            stride: data.len(),
            format: PixelFormat::Gray8,
            timestamp: Timestamp::new(0, 1, 30),
            display: Default::default(),
        };
        let measured = measure(&frame);
        assert_eq!((measured.width, measured.height), (256, 1));
        assert_eq!(measured.pixels, vec![[25; 3]; 256]);
    }

    #[test]
    fn padded_rectangular_sources_match_the_scalar_sampler_in_every_format() {
        for (w, h) in [(1, 513), (519, 1), (301, 167), (127, 83)] {
            for format in [
                PixelFormat::Gray8,
                PixelFormat::Rgb24,
                PixelFormat::Rgba8,
                PixelFormat::Bgra8,
            ] {
                let bpp = format.bytes_per_pixel();
                let stride = w * bpp + 5;
                // A decoder need not provide padding after the last visible row.
                let data: Vec<_> = (0..(h - 1) * stride + w * bpp)
                    .map(|i| (i % 251) as u8)
                    .collect();
                let frame = Frame {
                    data: &data,
                    width: w as u32,
                    height: h as u32,
                    stride,
                    format,
                    timestamp: Timestamp::new(0, 1, 1),
                    display: Default::default(),
                };
                let result = measure(&frame);
                let sw = result.width as usize;
                let sh = result.height as usize;
                for (index, &actual) in result.pixels.iter().enumerate() {
                    let x = (2 * (index % sw) + 1) * w / (2 * sw);
                    let y = (2 * (index / sw) + 1) * h / (2 * sh);
                    let offset = y * stride + x * bpp;
                    let bytes = &data[offset..offset + bpp];
                    let expected = match format {
                        PixelFormat::Gray8 => [bytes[0]; 3],
                        PixelFormat::Rgb24 | PixelFormat::Rgba8 => [bytes[0], bytes[1], bytes[2]],
                        PixelFormat::Bgra8 => [bytes[2], bytes[1], bytes[0]],
                    };
                    assert_eq!(actual, expected);
                }
            }
        }
    }

    #[test]
    fn centered_single_guest_does_not_split_at_upper_row_seams() {
        let image = scene(&[
            (1, 1, 50, 45, RED),
            (55, 1, 50, 45, GREEN),
            (109, 1, 50, 45, BLUE),
            (52, 50, 56, 45, GRAY),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 4);
        assert_box(
            &result.tiles[3],
            [52.0 / 160.0, 50.0 / 96.0, 56.0 / 160.0, 45.0 / 96.0],
        );
        assert_eq!(result.tiles[3].kind, TileKind::CameraOff);
        assert_eq!(result.tiles[3].role, TileRole::Primary);
    }

    #[test]
    fn flush_gallery_boundaries_and_clockwise_display_coordinates() {
        let mut image = scene(&[
            (0, 0, 80, 48, RED),
            (80, 0, 80, 48, GREEN),
            (0, 48, 80, 48, BLUE),
            (80, 48, 80, 48, GRAY),
        ]);
        image.rotation = Rotation::R90;
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 4);
        assert_box(&result.tiles[0], [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(result.tiles[2].kind, TileKind::CameraOff);
    }

    #[test]
    fn dominant_presentation_with_video_strip() {
        let image = scene(&[
            (0, 0, 120, 96, [225; 3]),
            (124, 8, 36, 38, RED),
            (124, 50, 36, 38, BLUE),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 3);
        assert_eq!(result.tiles[0].kind, TileKind::ScreenShare);
        assert_eq!(result.tiles[0].role, TileRole::Primary);
        assert_eq!(result.tiles[1].role, TileRole::Thumbnail);
        assert_eq!(result.tiles[2].role, TileRole::Thumbnail);
    }

    #[test]
    fn filmstrip_over_a_document_preserves_the_partial_stack() {
        let image = scene(&[
            (0, 0, 160, 96, [245; 3]),
            (10, 38, 52, 3, GRAY),
            (128, 0, 32, 18, RED),
            (128, 18, 32, 18, GREEN),
            (128, 36, 32, 18, BLUE),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 4);
        assert_eq!(result.tiles[0].kind, TileKind::ScreenShare);
        for (index, tile) in result.tiles[1..].iter().enumerate() {
            assert_eq!(tile.role, TileRole::Thumbnail);
            assert_box(tile, [0.8, (index * 18) as f32 / 96.0, 0.2, 18.0 / 96.0]);
        }
    }

    #[test]
    fn central_avatar_and_pillarboxed_video_have_distinct_backgrounds() {
        let image = scene(&[
            (0, 0, 78, 96, [36; 3]),
            (16, 25, 46, 46, RED),
            (82, 0, 78, 96, [36; 3]),
            (107, 0, 28, 96, GREEN),
        ]);
        let card = Bounds {
            left: 0,
            top: 0,
            right: 78,
            bottom: 96,
        };
        let portrait = Bounds {
            left: 82,
            top: 0,
            right: 160,
            bottom: 96,
        };
        assert_eq!(describe(&image, card).1, TileKind::CameraOff);
        assert_eq!(describe(&image, portrait).1, TileKind::Video);
    }

    #[test]
    fn descriptor_compares_same_view_across_sizes_and_different_feeds() {
        let first = detect(&scene(&[(1, 1, 70, 45, RED)]));
        let second = detect(&scene(&[(80, 5, 60, 70, RED)]));
        let other = detect(&scene(&[(80, 5, 60, 70, BLUE)]));
        let descriptor = |detection: &Detection| detection.tiles[0].descriptor.clone();
        assert_eq!(
            appearance_distance(&descriptor(&first), &descriptor(&second)),
            0.0
        );
        assert!(appearance_distance(&descriptor(&first), &descriptor(&other)) > 0.4);
    }

    #[test]
    fn black_gutter_touching_only_two_frame_edges_is_not_a_tile() {
        let result = detect(&scene(&[(0, 0, 76, 96, RED), (84, 0, 76, 96, BLUE)]));
        assert_eq!(result.tiles.len(), 2);
        assert_box(&result.tiles[0], [0.0, 0.0, 76.0 / 160.0, 1.0]);
        assert_box(&result.tiles[1], [84.0 / 160.0, 0.0, 76.0 / 160.0, 1.0]);
    }

    #[test]
    fn active_speaker_border_preserves_tile_bounds_and_appearance() {
        let before = detect(&scene(&[(0, 0, 78, 96, RED), (82, 0, 78, 96, BLUE)]));
        let after = detect(&scene(&[
            (0, 0, 78, 96, [230, 198, 28]),
            (2, 2, 74, 92, RED),
            (82, 0, 78, 96, BLUE),
        ]));
        assert_eq!(before.tiles.len(), 2);
        assert_eq!(after.tiles.len(), 2);
        assert_eq!(before.tiles[0].rect, after.tiles[0].rect);
        assert!(
            appearance_distance(&before.tiles[0].descriptor, &after.tiles[0].descriptor) < 0.10
        );
    }

    #[test]
    fn display_orientation_preserves_descriptors_for_asymmetric_tiles() {
        let width = 13;
        let height = 9;
        let mut upright = Measurements {
            width,
            height,
            pixels: Vec::new(),
            rotation: Rotation::R0,
        };
        for y in 0..height {
            for x in 0..width {
                upright.pixels.push([(x * 19) as u8, (y * 29) as u8, 100]);
            }
        }
        let expected = detect(&upright);
        let expected_grid = grid_feeds(&upright, 3, 2);
        for rotation in [Rotation::R90, Rotation::R180, Rotation::R270] {
            let (stored_width, stored_height) = match rotation {
                Rotation::R90 | Rotation::R270 => (height, width),
                _ => (width, height),
            };
            let mut rotated = Measurements {
                width: stored_width,
                height: stored_height,
                pixels: Vec::new(),
                rotation,
            };
            for y in 0..stored_height {
                for x in 0..stored_width {
                    let (display_x, display_y) = match rotation {
                        Rotation::R90 => (stored_height - 1 - y, x),
                        Rotation::R180 => (stored_width - 1 - x, stored_height - 1 - y),
                        Rotation::R270 => (y, stored_width - 1 - x),
                        Rotation::R0 => unreachable!(),
                    };
                    rotated
                        .pixels
                        .push(upright.at(display_x as usize, display_y as usize));
                }
            }
            let result = detect(&rotated);
            assert_eq!(result.tiles.len(), 1);
            assert_eq!(result.tiles[0].rect, expected.tiles[0].rect);
            assert_eq!(result.tiles[0].descriptor, expected.tiles[0].descriptor);
            let grid = grid_feeds(&rotated, 3, 2);
            for (actual, expected) in grid.tiles.iter().zip(&expected_grid.tiles) {
                assert_eq!(actual.rect, expected.rect);
                assert_eq!(actual.descriptor, expected.descriptor);
            }
        }
    }

    #[test]
    fn tiny_tile_histogram_counts_each_pixel_once() {
        let image = Measurements {
            width: 3,
            height: 1,
            pixels: vec![[150; 3], [190; 3], [190; 3]],
            rotation: Rotation::R0,
        };
        let result = detect(&image);
        assert_eq!(result.tiles[0].kind, TileKind::Video);
        let histogram = &result.tiles[0].descriptor[GRID_VALUES..];
        assert_eq!(histogram[150 / 32], 1.0 / 3.0);
        assert_eq!(histogram[190 / 32], 2.0 / 3.0);
    }

    #[test]
    fn black_frame_has_no_detected_tiles() {
        assert!(detect(&scene(&[])).tiles.is_empty());
    }
}
