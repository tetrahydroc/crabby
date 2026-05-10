//! Body-block decoding: metadata, identifiers, constants, maps, tokens.
//!
//! Entry point: [`parse`] takes a [`Frame`](crate::frame::Frame) and returns
//! a [`ParsedScript`] holding everything the reconstructor needs.

mod cursor;
mod identifiers;
mod maps;
mod tokens_stream;
mod variants;

use crabby_error::{CrabbyError, Result};

use crate::format::TokenizerVersion;
use crate::frame::Frame;
use crate::tokens::RawToken;

pub use self::cursor::Cursor;
pub use self::variants::Variant;

/// A fully decoded script ready for reconstruction.
#[derive(Debug)]
pub struct ParsedScript {
    /// Version the body was decoded under. Kept for downstream diagnostics
    /// (the differential test reports the version per fixture) even though
    /// the reconstructor is version-agnostic once decoding completes.
    #[allow(dead_code)]
    pub version: TokenizerVersion,
    pub identifiers: Vec<String>,
    pub constants: Vec<Variant>,
    /// Token index → 1-based source line number.
    pub line_map: Vec<(u32, u32)>,
    /// Token index → 0-based column number (indent units are 4 cols = 1 tab).
    pub col_map: Vec<(u32, u32)>,
    pub tokens: Vec<RawToken>,
}

/// Decode the full body of a parsed frame.
pub fn parse(frame: &Frame) -> Result<ParsedScript> {
    let meta_size = frame.version.meta_block_size();
    if frame.body.len() < meta_size {
        return Err(CrabbyError::Detokenize {
            context: format!(
                "body too short for meta block: got {}, need {meta_size}",
                frame.body.len(),
            ),
            source: "truncated GDSC body".into(),
        });
    }

    let mut cursor = Cursor::new(&frame.body);

    // Metadata block. Layout differs v100 (with padding) vs v101.
    let ident_count = cursor.read_u32_at(0)?;
    let const_count = cursor.read_u32_at(4)?;
    let line_count = cursor.read_u32_at(8)?;
    let token_count = cursor.read_u32_at(frame.version.token_count_offset())?;
    cursor.advance_to(meta_size);

    let identifiers = identifiers::decode(&mut cursor, ident_count)?;
    let constants = variants::decode_sequence(&mut cursor, const_count)?;
    let line_map = maps::decode(&mut cursor, line_count)?;
    let col_map = maps::decode(&mut cursor, line_count)?;
    let tokens = tokens_stream::decode(&mut cursor, token_count)?;

    Ok(ParsedScript {
        version: frame.version,
        identifiers,
        constants,
        line_map,
        col_map,
        tokens,
    })
}
