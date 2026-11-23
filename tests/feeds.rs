use tiletrail::*;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 96;
type Picture = (u32, u32, u32, u32, [u8; 3]);

fn time(pts: i64) -> Timestamp {
    Timestamp::new(pts, 1, 30)
}

fn source(background: [u8; 3], pictures: &[Picture]) -> Vec<u8> {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let pixel = pictures
                .iter()
                .find(|&&(left, top, right, bottom, _)| {
                    (left..right).contains(&x) && (top..bottom).contains(&y)
                })
                .map_or(background, |&(_, _, _, _, color)| {
                    let texture = (x % 5 + y % 7) as u8;
                    color.map(|channel| channel.saturating_sub(texture))
                });
            data.extend_from_slice(&pixel);
        }
    }
    data
}

fn frame(data: &[u8], timestamp: i64, rotation: Rotation) -> Frame<'_> {
    Frame {
        data,
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH as usize * 3,
        format: PixelFormat::Rgb24,
        timestamp: time(timestamp),
        display: DisplayGeometry {
            rotation,
            ..DisplayGeometry::default()
        },
    }
}

fn config() -> AnalyzerConfig {
    AnalyzerConfig {
        mode: BackendMode::CpuOnly,
        sampling: Sampling::EveryFrame,
    }
}

#[test]
fn automatic_finds_asymmetric_feeds_and_adapts_to_a_borderless_layout() {
    let mut analyzer = Analyzer::with_layout(config(), time(0), FeedLayout::Automatic).unwrap();
    let white = [240, 235, 240];
    let red = [180, 30, 54];
    let green = [30, 195, 108];
    let colored_gutters = source(
        [114, 24, 184],
        &[
            (0, 0, 101, HEIGHT, white),
            (105, 0, WIDTH, 36, red),
            (105, 40, WIDTH, HEIGHT, green),
        ],
    );
    let borderless = source(
        [0; 3],
        &[(0, 0, 76, HEIGHT, green), (76, 0, WIDTH, HEIGHT, white)],
    );
    analyzer
        .push(frame(&colored_gutters, 0, Rotation::R0))
        .unwrap();
    analyzer.push(frame(&borderless, 1, Rotation::R0)).unwrap();
    let result = analyzer.finish(time(2)).unwrap();
    let first = &result.visible_at(time(0)).unwrap().regions;
    let second = &result.visible_at(time(1)).unwrap().regions;
    assert_eq!(first.len(), 3, "{first:?}");
    assert_eq!(second.len(), 2, "{second:?}");
    assert_eq!(first[0].rect.width, 101.0 / WIDTH as f32);
    assert_eq!(first[1].rect.x, 105.0 / WIDTH as f32);
    assert_eq!(first[2].rect.y, 40.0 / HEIGHT as f32);
    assert_eq!(second[0].rect.width, 76.0 / WIDTH as f32);
    assert!(
        first
            .iter()
            .chain(second)
            .all(|region| { region.kind == TileKind::Video && region.role == TileRole::Primary })
    );
    assert_eq!(first[0].feed_id(), second[1].feed_id());
    assert_eq!(first[2].feed_id(), second[0].feed_id());
}

#[test]
fn grid_tracks_display_cells_through_black_identical_and_rearranged_frames() {
    let layout = FeedLayout::Grid {
        columns: 3,
        rows: 2,
    };
    let mut analyzer = Analyzer::with_layout(config(), time(0), layout).unwrap();
    assert_eq!(analyzer.layout(), layout);
    let different = source([30, 100, 207], &[(0, 0, WIDTH, HEIGHT / 2, [215, 32, 71])]);
    let black = source([0; 3], &[]);
    let identical = source([140; 3], &[]);
    for (index, data) in [&different, &black, &identical, &different]
        .into_iter()
        .enumerate()
    {
        analyzer
            .push(frame(data, index as i64, Rotation::R90))
            .unwrap();
    }
    let result = analyzer.finish(time(4)).unwrap();
    assert_eq!(result.feed_tracks().len(), 6);
    assert_eq!(result.appearances.len(), 6);
    assert!(!result.events.is_empty());
    assert!(
        result
            .events
            .iter()
            .all(|event| event.kind == ChangeKind::HardCut)
    );
    for observation in &result.observations {
        assert_eq!(
            result.coverage_at(observation.timestamp),
            Some(Coverage::Observed)
        );
        assert_eq!(observation.regions.len(), 6);
        for (index, region) in observation.regions.iter().enumerate() {
            assert_eq!(region.feed_id(), Some(index as FeedId + 1));
            assert_eq!(region.appearance_id, index as AppearanceId + 1);
            assert_eq!(region.identity_confidence, Some(1.0));
            assert!(!region.identity_uncertain);
            assert_eq!(
                region.rect,
                Rect {
                    x: (index % 3) as f32 / 3.0,
                    y: (index / 3) as f32 / 2.0,
                    width: 1.0 / 3.0,
                    height: 0.5,
                }
            );
        }
    }
    let appearance = result.feed_appearances(6, time(0), time(4)).next().unwrap();
    assert_eq!(appearance.feed_id(), Some(6));
    assert_eq!(appearance.end, time(4));
}

#[test]
fn grid_limits_dimensions_and_accepts_a_coarse_measurement() {
    for (columns, rows) in [(0, 1), (64, 2), (u32::MAX, u32::MAX)] {
        let error = Analyzer::with_layout(config(), time(0), FeedLayout::Grid { columns, rows })
            .err()
            .unwrap();
        assert_eq!(error.kind, ErrorKind::InvalidRange);
    }
    let layout = FeedLayout::Grid {
        columns: MAX_GRID_FEEDS,
        rows: 1,
    };
    let mut analyzer = Analyzer::with_layout(config(), time(0), layout).unwrap();
    analyzer
        .push(Frame {
            data: &[0],
            width: 1,
            height: 1,
            stride: 1,
            format: PixelFormat::Gray8,
            timestamp: time(0),
            display: DisplayGeometry::default(),
        })
        .unwrap();
    let result = analyzer.finish(time(1)).unwrap();
    assert_eq!(result.feed_tracks().len(), MAX_GRID_FEEDS as usize);
}

#[test]
fn automatic_recovers_a_soft_seam_when_the_other_row_supports_it() {
    let mut analyzer = Analyzer::with_layout(config(), time(0), FeedLayout::Automatic).unwrap();
    let soft_pair = [
        (0, 0, 79, 48, [125, 90, 201]),
        (79, 0, 80, 48, [156, 121, 201]),
        (80, 0, 160, 48, [187, 152, 201]),
    ];
    let mut grid = soft_pair.to_vec();
    grid.extend([
        (0, 48, 80, 96, [25, 210, 33]),
        (80, 48, 160, 96, [48, 32, 21]),
    ]);
    let add_gradient = |mut data: Vec<u8>| {
        for (y, row) in data.chunks_exact_mut(WIDTH as usize * 3).enumerate() {
            for channel in row {
                *channel = channel.saturating_add((y / 2) as u8);
            }
        }
        data
    };
    let grid = add_gradient(source([0; 3], &grid));
    let solo = add_gradient(source(
        [0; 3],
        &soft_pair.map(|(left, top, right, _, color)| (left, top, right, HEIGHT, color)),
    ));
    analyzer.push(frame(&grid, 0, Rotation::R0)).unwrap();
    analyzer.push(frame(&solo, 1, Rotation::R0)).unwrap();
    let result = analyzer.finish(time(2)).unwrap();
    let four = &result.visible_at(time(0)).unwrap().regions;
    assert_eq!(four.len(), 4, "{four:?}");
    assert!(
        four.iter()
            .all(|tile| tile.rect.width == 0.5 && tile.rect.height == 0.5)
    );
    assert_eq!(result.visible_at(time(1)).unwrap().regions.len(), 1);
}
