//! Find a filmstrip of adjacent avatar cards from their solid backgrounds.
//! The nearly equal widths, common inner edge, and centered foreground distinguish
//! the card surfaces from an unframed row of controls or participant badges.
use super::{Bounds, Measurements, color_gap};

#[derive(Clone, Copy)]
struct Card {
    start: usize,
    end: usize,
    background: [u8; 3],
}

impl Card {
    fn width(self) -> usize {
        self.end - self.start
    }
}

fn bounds_for_run(
    image: &Measurements,
    frame: Bounds,
    horizontal: bool,
    trailing: bool,
    cards: &[Card],
) -> Option<(Bounds, Vec<Bounds>)> {
    if cards.len() < 3 {
        return None;
    }
    let (_, extent, _, _) = frame.dimensions(horizontal);
    let line = |distance| {
        if trailing {
            extent - 1 - distance
        } else {
            distance
        }
    };
    let supported = |distance| {
        let mut votes = 0;
        for card in cards {
            for offset in [1, 7] {
                let cross = card.start + card.width() * offset / 8;
                votes += usize::from(
                    color_gap(
                        image.at_line(horizontal, line(distance), cross),
                        card.background,
                    ) <= 3,
                );
            }
        }
        votes * 3 >= cards.len() * 4
    };
    let height = (3..extent / 3).find(|&d| !supported(d) && !supported(d + 1))?;
    if height < 6 || height * 4 < cards[0].width() || height > cards[0].width() * 3 {
        return None;
    }
    // Each card must contain a foreground icon in the middle of its otherwise
    // matching background. A row of flat swatches or a menu does not suffice.
    if !cards.iter().all(|card| {
        [3, 4, 5].into_iter().any(|h| {
            [3, 4, 5].into_iter().any(|w| {
                let pixel = image.at_line(
                    horizontal,
                    line(height * h / 8),
                    card.start + card.width() * w / 8,
                );
                color_gap(pixel, card.background) > 18
            })
        })
    }) {
        return None;
    }
    let (strip, stage) = if trailing {
        (
            frame.with_extent(horizontal, extent - height, extent),
            frame.with_extent(horizontal, 0, extent - height),
        )
    } else {
        (
            frame.with_extent(horizontal, 0, height),
            frame.with_extent(horizontal, height, extent),
        )
    };
    Some((
        stage,
        cards
            .iter()
            .map(|card| strip.with_extent(!horizontal, card.start, card.end))
            .collect(),
    ))
}

pub(super) fn find(image: &Measurements, frame: Bounds) -> Option<(Bounds, Vec<Bounds>)> {
    for horizontal in [true, false] {
        let (_, extent, _, length) = frame.dimensions(horizontal);
        if extent < 24 || length < 24 {
            continue;
        }
        for trailing in [false, true] {
            let line = if trailing { extent - 1 } else { 0 };
            let mut cards = Vec::<Card>::new();
            let mut start = 0;
            while start < length {
                let background = image.at_line(horizontal, line, start);
                let mut end = start + 1;
                while end < length
                    && color_gap(image.at_line(horizontal, line, end), background) <= 2
                {
                    end += 1;
                }
                let card = Card {
                    start,
                    end,
                    background,
                };
                let high = *background.iter().max().unwrap();
                let low = *background.iter().min().unwrap();
                let flat_card = high < 128 && high - low <= 12 && card.width() >= 6;
                if !flat_card
                    || cards
                        .last()
                        .is_some_and(|prev| prev.width().abs_diff(card.width()) * 5 > prev.width())
                {
                    if let Some(strip) = bounds_for_run(image, frame, horizontal, trailing, &cards)
                    {
                        return Some(strip);
                    }
                    cards.clear();
                }
                if flat_card {
                    cards.push(card);
                }
                start = end;
            }
            if let Some(strip) = bounds_for_run(image, frame, horizontal, trailing, &cards) {
                return Some(strip);
            }
        }
    }
    None
}
