//! Versioned chunks of a paused analyzer. The caller must bind these to the
//! same source and make a chunk durable before advancing its cursor.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::tracker::journal::{Chunk, SavedChunk};
use crate::{Analyzer, BackendDiagnostic, BackendMode, FeedLayout, Sampling, Timestamp};

const JOURNAL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub(crate) observations: usize,
    pub(crate) events: usize,
    pub(crate) coarse: usize,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Header {
    version: u32,
    start: Timestamp,
    mode: BackendMode,
    sampling: Sampling,
    layout: FeedLayout,
    previous: Cursor,
}

#[derive(Serialize)]
struct Entry<'a> {
    header: Header,
    tracker: Chunk<'a>,
    coarse: &'a [usize],
    diagnostics: &'a [BackendDiagnostic],
}

#[derive(Deserialize)]
struct SavedEntry {
    header: Header,
    tracker: SavedChunk,
    coarse: Vec<usize>,
    diagnostics: Vec<BackendDiagnostic>,
}

impl Analyzer {
    pub(crate) fn journal_cursor(&self) -> Cursor {
        Cursor {
            observations: self.tracker.observations.len(),
            events: self.tracker.events.len(),
            coarse: self.coarse.len(),
        }
    }

    fn journal_header(&self, previous: Cursor) -> Header {
        Header {
            version: JOURNAL_VERSION,
            start: self.start,
            mode: self.config.mode,
            sampling: self.config.sampling,
            layout: self.layout,
            previous,
        }
    }

    pub(crate) fn write_checkpoint(&self, previous: Cursor, writer: impl Write) -> io::Result<()> {
        let current = self.journal_cursor();
        if self.uncommitted.is_some()
            || current.observations <= previous.observations
            || current.events < previous.events
            || current.coarse < previous.coarse
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid checkpoint cursor",
            ));
        }
        serde_json::to_writer(
            writer,
            &Entry {
                header: self.journal_header(previous),
                tracker: self
                    .tracker
                    .journal_chunk(previous.observations, previous.events),
                coarse: &self.coarse[previous.coarse..],
                diagnostics: self.diagnostics(),
            },
        )?;
        Ok(())
    }

    pub(crate) fn read_checkpoint(&mut self, reader: impl Read) -> io::Result<()> {
        let entry: SavedEntry = serde_json::from_reader(reader)?;
        let cursor = self.journal_cursor();
        if entry.header != self.journal_header(cursor)
            || entry.diagnostics.last().map(|d| d.backend) != Some(self.backend())
            || self.uncommitted.is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "checkpoint configuration or sequence changed",
            ));
        }
        self.tracker.append_journal_chunk(entry.tracker);
        self.coarse.extend(entry.coarse);
        self.session.restore_diagnostics(entry.diagnostics);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalyzerConfig, CoarseOutcome, DisplayGeometry, Frame, PixelFormat};

    #[test]
    fn replaying_journal_chunks_restores_live_tracking() {
        let time = |pts| Timestamp::new(pts, 1, 1000);
        let config = AnalyzerConfig {
            mode: BackendMode::CpuOnly,
            sampling: Sampling::EveryFrame,
        };
        let layout = FeedLayout::Grid {
            columns: 2,
            rows: 1,
        };
        let mut original = Analyzer::with_layout(config, time(0), layout).unwrap();
        let mut resumed = Analyzer::with_layout(config, time(0), layout).unwrap();
        let mut cursor = Cursor::default();
        let pixels: Vec<u8> = (0..64).map(|index| (index * 19) as u8).collect();
        let frame = |pts| Frame {
            data: &pixels,
            width: 8,
            height: 8,
            stride: 8,
            format: PixelFormat::Gray8,
            timestamp: time(pts),
            display: DisplayGeometry::default(),
        };
        for pts in [0, 40, 80, 120] {
            if pts == 80 {
                assert_eq!(
                    original.push_coarse(frame(pts)).unwrap(),
                    CoarseOutcome::Committed
                );
            } else {
                original.push(frame(pts)).unwrap();
            }
            let mut bytes = Vec::new();
            original.write_checkpoint(cursor, &mut bytes).unwrap();
            if cursor.observations != 0 {
                let mut wrong_prefix = Analyzer::with_layout(config, time(0), layout).unwrap();
                assert!(wrong_prefix.read_checkpoint(bytes.as_slice()).is_err());
            }
            resumed.read_checkpoint(bytes.as_slice()).unwrap();
            cursor = original.journal_cursor();
        }
        original.push_coarse(frame(200)).unwrap();
        resumed.push_coarse(frame(200)).unwrap();
        original.push(frame(220)).unwrap();
        resumed.push(frame(220)).unwrap();
        assert_eq!(
            serde_json::to_vec(&original.snapshot(time(240)).unwrap()).unwrap(),
            serde_json::to_vec(&resumed.snapshot(time(240)).unwrap()).unwrap(),
        );
    }
}
