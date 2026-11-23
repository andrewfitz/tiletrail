use tiletrail::*;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 96;

#[derive(Clone, Copy)]
struct Picture {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    trim: u32,
    color: [u8; 3],
    trim_color: [u8; 3],
}

const FIRST: Picture = Picture {
    left: 11,
    top: 12,
    right: 54,
    bottom: 42,
    trim: 2,
    color: [185, 53, 48],
    trim_color: [24; 3],
};
const SECOND: Picture = Picture {
    left: 107,
    top: 51,
    right: 150,
    bottom: 87,
    trim: 1,
    color: [51, 158, 93],
    trim_color: [226, 205, 19],
};
const BACKGROUND: [u8; 3] = [61, 81, 154];
const FULL_FRAME: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 1.0,
    height: 1.0,
};

fn texture(x: u32, y: u32, step: u32, color: [u8; 3]) -> [u8; 3] {
    // Moving features with gradual edges avoid creating synthetic feed seams.
    let cross = f64::from(x + 2 * y + step * 4) / 11.0;
    let diagonal = (f64::from(x + step * 2) - 2.0 * f64::from(y)) / 7.0;
    let shade = (cross.sin() * 29.0 + diagonal.cos() * 15.0) as i32;
    [
        (i32::from(color[0]) + shade).clamp(0, 255) as u8,
        (i32::from(color[1]) + shade / 2).clamp(0, 255) as u8,
        (i32::from(color[2]) - shade / 2).clamp(0, 255) as u8,
    ]
}

fn source(step: u32, pictures: &[Picture], break_trim: bool, flat_background: bool) -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let base = if flat_background {
                BACKGROUND
            } else {
                texture(x, y, step, BACKGROUND)
            };
            let rgb = pictures
                .iter()
                .find(|p| (p.left..p.right).contains(&x) && (p.top..p.bottom).contains(&y))
                .map_or(base, |p| {
                    if (p.left + p.trim..p.right - p.trim).contains(&x)
                        && (p.top + p.trim..p.bottom - p.trim).contains(&y)
                    {
                        texture(x, y, step * 2, p.color)
                    } else if break_trim && x == p.left + 7 {
                        base
                    } else {
                        p.trim_color
                    }
                });
            data.extend(rgb);
        }
    }
    data
}

fn timestamp(step: u32) -> Timestamp {
    Timestamp::new(i64::from(step), 1, 30)
}

fn analyze(frames: &[Vec<u8>], rotation: Rotation) -> AnalysisResult {
    let config = AnalyzerConfig {
        mode: BackendMode::CpuOnly,
        sampling: Sampling::EveryFrame,
    };
    let mut analyzer = Analyzer::with_layout(config, timestamp(0), FeedLayout::Automatic).unwrap();
    for (step, data) in frames.iter().enumerate() {
        analyzer
            .push(Frame {
                data,
                width: WIDTH,
                height: HEIGHT,
                stride: WIDTH as usize * 3,
                format: PixelFormat::Rgb24,
                timestamp: timestamp(step as u32),
                display: DisplayGeometry {
                    rotation,
                    ..DisplayGeometry::default()
                },
            })
            .unwrap();
    }
    analyzer.finish(timestamp(frames.len() as u32)).unwrap()
}

fn display_rect(picture: Picture, rotation: Rotation) -> Rect {
    let width = (picture.right - picture.left - 2 * picture.trim) as f32 / WIDTH as f32;
    let height = (picture.bottom - picture.top - 2 * picture.trim) as f32 / HEIGHT as f32;
    match rotation {
        Rotation::R0 => Rect {
            x: (picture.left + picture.trim) as f32 / WIDTH as f32,
            y: (picture.top + picture.trim) as f32 / HEIGHT as f32,
            width,
            height,
        },
        Rotation::R90 => Rect {
            x: (HEIGHT - picture.bottom + picture.trim) as f32 / HEIGHT as f32,
            y: (picture.left + picture.trim) as f32 / WIDTH as f32,
            width: height,
            height: width,
        },
        _ => unreachable!(),
    }
}

#[test]
fn automatic_tracks_two_framed_insets_in_changing_footage() {
    let frames: Vec<_> = (0..3)
        .map(|step| source(step, &[FIRST, SECOND], false, false))
        .collect();
    for rotation in [Rotation::R0, Rotation::R90] {
        let result = analyze(&frames, rotation);
        for picture in [FIRST, SECOND] {
            let expected = display_rect(picture, rotation);
            let mut feed = None;
            for step in 0..3 {
                let regions = &result.visible_at(timestamp(step)).unwrap().regions;
                assert_eq!(regions.len(), 3, "{regions:?}");
                assert!(regions.iter().any(|region| region.rect == FULL_FRAME));
                let region = regions
                    .iter()
                    .find(|region| region.rect == expected)
                    .unwrap();
                assert_eq!(region.kind, TileKind::Video);
                if step == 0 {
                    feed = region.feed_id();
                    assert!(feed.is_some());
                }
                assert_eq!(region.feed_id(), feed);
            }
        }
    }
}

#[test]
fn automatic_ignores_broken_trim_and_same_scene_through_a_frame() {
    let same_scene = Picture {
        color: BACKGROUND,
        ..FIRST
    };
    let frames = [
        source(0, &[], false, false),
        source(1, &[same_scene], false, false),
        source(2, &[FIRST], true, false),
        source(3, &[FIRST], false, true),
    ];
    let result = analyze(&frames, Rotation::R0);
    for step in 0..frames.len() as u32 {
        let regions = &result.visible_at(timestamp(step)).unwrap().regions;
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert_eq!(regions[0].rect, FULL_FRAME);
    }
}

#[test]
fn automatic_keeps_inset_geometry_with_perturbed_trim_and_low_contrast_background() {
    let mut image = source(0, &[FIRST], false, false);
    let trim = FIRST;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let i = ((y * WIDTH + x) * 3) as usize;
            let pixel = &mut image[i..i + 3];
            let inside_outer =
                (trim.left..trim.right).contains(&x) && (trim.top..trim.bottom).contains(&y);
            if !inside_outer {
                // A textured background can have a narrow range near the inset.
                pixel[0] = 96 + pixel[0] / 2;
                pixel[1] = 102 + pixel[1] / 3;
                pixel[2] = 125 + pixel[2] / 3;
            } else if x < trim.left + trim.trim
                || x >= trim.right - trim.trim
                || y < trim.top + trim.trim
                || y >= trim.bottom - trim.trim
            {
                // Slight blockwise color shifts and a larger isolated error
                // model the decoded trim pixels in an encoded composite.
                let shift = ((x / 4 + y / 4) % 9) as u8;
                pixel[0] += shift;
                pixel[1] += 2 * shift;
                if x == trim.left + 1 && y == trim.top + 7 {
                    pixel[2] += 24;
                }
            }
        }
    }

    let result = analyze(&[image], Rotation::R0);
    let regions = &result.visible_at(timestamp(0)).unwrap().regions;
    assert_eq!(regions.len(), 2, "{regions:?}");
    assert!(
        regions
            .iter()
            .any(|r| r.rect == display_rect(trim, Rotation::R0))
    );
}

#[test]
fn automatic_keeps_bordered_inset_inside_one_of_two_feeds() {
    let mut image = source(0, &[FIRST], false, false);
    let middle = WIDTH / 2;
    for y in 0..HEIGHT {
        for x in middle..WIDTH {
            let i = ((y * WIDTH + x) * 3) as usize;
            image[i..i + 3].copy_from_slice(&texture(x, y, 0, [225, 191, 78]));
        }
    }
    let result = analyze(&[image], Rotation::R0);
    let regions = &result.visible_at(timestamp(0)).unwrap().regions;
    assert_eq!(regions.len(), 3, "{regions:?}");
    for expected in [
        Rect {
            width: 0.5,
            ..FULL_FRAME
        },
        Rect {
            x: 0.5,
            width: 0.5,
            ..FULL_FRAME
        },
        display_rect(FIRST, Rotation::R0),
    ] {
        assert!(regions.iter().any(|r| r.rect == expected));
    }
}
