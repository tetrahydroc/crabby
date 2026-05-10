//! Godot Variant decoder for the GDSC constant table.
//!
//! Constants are stored as a flat sequence of Godot `Variant`-encoded values.
//! Each value starts with a 4-byte type header (low 3 bytes = type tag, high
//! byte = encoding flags), followed by a type-dependent body padded to a
//! 4-byte boundary.
//!
//! Reference: Godot's `core/io/marshalls.cpp::decode_variant`. Only the
//! subset of types that legitimately appear in `.gdc` constant
//! tables is decoded, numeric literals, strings, and the small composite
//! types scripts write as literals. Anything unexpected errors loudly so
//! new constant types surface through failure rather than silent truncation.

use crabby_error::{CrabbyError, Result};

use super::Cursor;

/// Flag bit on the type header indicating 64-bit width for INT/FLOAT.
const ENCODE_FLAG_64: u32 = 1 << 16;

// Variant type tags. Godot assigns these in `Variant::Type` in
// `core/variant/variant.h`. We list only the ones we decode.
const TYPE_NIL: u32 = 0;
const TYPE_BOOL: u32 = 1;
const TYPE_INT: u32 = 2;
const TYPE_FLOAT: u32 = 3;
const TYPE_STRING: u32 = 4;
const TYPE_STRING_NAME: u32 = 21;
const TYPE_NODE_PATH: u32 = 22;

/// The subset of Godot Variants we model. Extended as fixture scripts need it.
#[derive(Debug, Clone)]
pub enum Variant {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringName(String),
    NodePath(String),
}

/// Decode `count` sequential variants from `cursor`.
pub fn decode_sequence(cursor: &mut Cursor<'_>, count: u32) -> Result<Vec<Variant>> {
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        out.push(decode_one(cursor, i).map_err(|e| annotate(i, e))?);
    }
    Ok(out)
}

fn annotate(index: u32, err: CrabbyError) -> CrabbyError {
    match err {
        CrabbyError::Detokenize { context, source } => CrabbyError::Detokenize {
            context: format!("variant {index}: {context}"),
            source,
        },
        other => other,
    }
}

fn decode_one(cursor: &mut Cursor<'_>, _index: u32) -> Result<Variant> {
    let header = cursor.read_u32()?;
    let type_tag = header & 0xffff;
    let flags = header & !0xffff;

    match type_tag {
        TYPE_NIL => Ok(Variant::Nil),
        TYPE_BOOL => decode_bool(cursor),
        TYPE_INT => decode_int(cursor, flags),
        TYPE_FLOAT => decode_float(cursor, flags),
        TYPE_STRING => decode_string(cursor).map(Variant::String),
        TYPE_STRING_NAME => decode_string(cursor).map(Variant::StringName),
        TYPE_NODE_PATH => decode_string(cursor).map(Variant::NodePath),
        other => Err(CrabbyError::Detokenize {
            context: format!("unsupported variant type tag {other} (flags=0x{flags:08x})"),
            source: "extend variants.rs to cover this type".into(),
        }),
    }
}

fn decode_bool(cursor: &mut Cursor<'_>) -> Result<Variant> {
    let v = cursor.read_u32()?;
    Ok(Variant::Bool(v != 0))
}

fn decode_int(cursor: &mut Cursor<'_>, flags: u32) -> Result<Variant> {
    if flags & ENCODE_FLAG_64 != 0 {
        let lo = u64::from(cursor.read_u32()?);
        let hi = u64::from(cursor.read_u32()?);
        let raw = lo | (hi << 32);
        #[allow(clippy::cast_possible_wrap)] // Godot stores i64 bit-pattern as u64
        Ok(Variant::Int(raw as i64))
    } else {
        let raw = cursor.read_u32()?;
        #[allow(clippy::cast_possible_wrap)] // Godot stores i32 bit-pattern as u32
        Ok(Variant::Int(i64::from(raw as i32)))
    }
}

fn decode_float(cursor: &mut Cursor<'_>, flags: u32) -> Result<Variant> {
    if flags & ENCODE_FLAG_64 != 0 {
        let lo = u64::from(cursor.read_u32()?);
        let hi = u64::from(cursor.read_u32()?);
        Ok(Variant::Float(f64::from_bits(lo | (hi << 32))))
    } else {
        let raw = cursor.read_u32()?;
        Ok(Variant::Float(f64::from(f32::from_bits(raw))))
    }
}

/// Decode a length-prefixed UTF-8 string. Godot pads each string body up to
/// a 4-byte boundary with zero bytes.
fn decode_string(cursor: &mut Cursor<'_>) -> Result<String> {
    let raw_len = cursor.read_u32()? as usize;
    let bytes = cursor.take(raw_len)?.to_vec();
    let padding = raw_len.next_multiple_of(4) - raw_len;
    if padding > 0 {
        let _ = cursor.take(padding)?;
    }
    String::from_utf8(bytes).map_err(|source| CrabbyError::Detokenize {
        context: format!("string ({raw_len} bytes) is not valid UTF-8"),
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a type header with the given tag and flags.
    fn header(tag: u32, flags_high: u32) -> [u8; 4] {
        (tag | (flags_high << 16)).to_le_bytes()
    }

    #[test]
    fn decodes_nil() {
        let bytes = header(TYPE_NIL, 0);
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 1).unwrap();
        assert!(matches!(v[0], Variant::Nil));
    }

    #[test]
    fn decodes_bool_true_and_false() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_BOOL, 0));
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&header(TYPE_BOOL, 0));
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 2).unwrap();
        assert!(matches!(v[0], Variant::Bool(true)));
        assert!(matches!(v[1], Variant::Bool(false)));
    }

    #[test]
    fn decodes_int_32_bit() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_INT, 0));
        bytes.extend_from_slice(&(-42i32).to_le_bytes());
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 1).unwrap();
        match &v[0] {
            Variant::Int(n) => assert_eq!(*n, -42),
            other => panic!("expected Int, got {other:?}"),
        }
    }

    #[test]
    fn decodes_int_64_bit() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_INT, 1));
        bytes.extend_from_slice(&(-1i64).to_le_bytes());
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 1).unwrap();
        match &v[0] {
            Variant::Int(n) => assert_eq!(*n, -1),
            other => panic!("expected Int, got {other:?}"),
        }
    }

    #[test]
    fn decodes_float_32_and_64() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_FLOAT, 0));
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&header(TYPE_FLOAT, 1));
        bytes.extend_from_slice(&2.5f64.to_le_bytes());
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 2).unwrap();
        match &v[0] {
            Variant::Float(f) => assert!((f - 1.5).abs() < 1e-6),
            other => panic!("expected Float, got {other:?}"),
        }
        match &v[1] {
            Variant::Float(f) => assert!((f - 2.5).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn decodes_string_with_padding() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_STRING, 0));
        // "hello" = 5 bytes; padded to 8 with 3 zero bytes.
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        bytes.extend_from_slice(&[0, 0, 0]);
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 1).unwrap();
        match &v[0] {
            Variant::String(s) => assert_eq!(s, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn decodes_string_name_and_node_path() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(TYPE_STRING_NAME, 0));
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"foo");
        bytes.push(0); // pad
        bytes.extend_from_slice(&header(TYPE_NODE_PATH, 0));
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(b"/Bar");
        let mut c = Cursor::new(&bytes);
        let v = decode_sequence(&mut c, 2).unwrap();
        match &v[0] {
            Variant::StringName(s) => assert_eq!(s, "foo"),
            other => panic!("expected StringName, got {other:?}"),
        }
        match &v[1] {
            Variant::NodePath(s) => assert_eq!(s, "/Bar"),
            other => panic!("expected NodePath, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_tag_errors_with_context() {
        let bytes = header(999, 0);
        let mut c = Cursor::new(&bytes);
        let err = decode_sequence(&mut c, 1).expect_err("should fail");
        match err {
            CrabbyError::Detokenize { context, .. } => {
                assert!(context.contains("variant 0"));
                assert!(context.contains("999"), "got: {context}");
            }
            other => panic!("expected Detokenize, got {other:?}"),
        }
    }
}
