//! Line-map and column-map decoder.
//!
//! Both maps have identical shape: `line_count` records, each
//! `(token_index: u32, value: u32)`. The line map associates a token with
//! its source-line number; the column map associates it with its column.

use crabby_error::Result;

use super::Cursor;

pub fn decode(cursor: &mut Cursor<'_>, count: u32) -> Result<Vec<(u32, u32)>> {
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let key = cursor.read_u32()?;
        let val = cursor.read_u32()?;
        out.push((key, val));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pairs() {
        let mut bytes = Vec::new();
        for (k, v) in [(0u32, 1u32), (5, 2), (12, 3)] {
            bytes.extend_from_slice(&k.to_le_bytes());
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let mut c = Cursor::new(&bytes);
        assert_eq!(decode(&mut c, 3).unwrap(), vec![(0, 1), (5, 2), (12, 3)]);
    }

    #[test]
    fn empty_map_yields_empty_vec() {
        let mut c = Cursor::new(&[]);
        assert_eq!(decode(&mut c, 0).unwrap(), Vec::<(u32, u32)>::new());
    }
}
