//! Token-type table + spacing rules.
//!
//! Index assignments are Godot's v101 tokenizer indices (same as vostok's
//! `_TOKEN_TEXT`). Comments in the lookup list each token's source form.
//! See `../format.rs` for the raw version bits; the table below lists which
//! token kind each numeric index represents.

/// Raw token as emitted by the decoder before reconstruction.
#[derive(Debug, Clone, Copy)]
pub struct RawToken {
    /// Low-7-bit token-type tag (0..=99).
    pub kind: u32,
    /// Data-table index. Meaning depends on `kind`:
    /// - IDENTIFIER / ANNOTATION → index into identifier table
    /// - LITERAL → index into constants table
    /// - other → unused
    pub data_index: u32,
}

// --- Named token kinds we reference outside this module ------------------

pub const TK_ANNOTATION: u32 = 1;
pub const TK_IDENTIFIER: u32 = 2;
pub const TK_LITERAL: u32 = 3;
pub const TK_NOT_WORD: u32 = 12; // "not"
pub const TK_BANG: u32 = 15; // "!"
pub const TK_TILDE: u32 = 18; // "~"
pub const TK_PAREN_OPEN: u32 = 77;
pub const TK_BRACKET_OPEN: u32 = 73;
pub const TK_BRACKET_CLOSE: u32 = 74;
pub const TK_BRACE_CLOSE: u32 = 76;
pub const TK_PAREN_CLOSE: u32 = 78;
pub const TK_DOT: u32 = 81;
pub const TK_DOLLAR: u32 = 85;
pub const TK_UNDERSCORE: u32 = 87;
pub const TK_NEWLINE: u32 = 88;
pub const TK_INDENT: u32 = 89;
pub const TK_DEDENT: u32 = 90;
pub const TK_PI: u32 = 91;
pub const TK_TAU: u32 = 92;
pub const TK_INF: u32 = 93;
pub const TK_NAN: u32 = 94;
pub const TK_EOF: u32 = 99;

pub const FIRST_KEYWORD: u32 = 40;
pub const LAST_KEYWORD: u32 = 72;
pub const FIRST_CONTROL_FLOW_KW: u32 = 40; // if..when
pub const LAST_CONTROL_FLOW_KW: u32 = 50;

/// Static text for punctuation + keywords indexed by token kind.
///
/// `None` → the token has no fixed text (EMPTY/IDENTIFIER/LITERAL/etc., or
/// not a token we emit).
#[must_use]
pub const fn token_text(kind: u32) -> Option<&'static str> {
    match kind {
        4 => Some("<"),
        5 => Some("<="),
        6 => Some(">"),
        7 => Some(">="),
        8 => Some("=="),
        9 => Some("!="),
        10 => Some("and"),
        11 => Some("or"),
        12 => Some("not"),
        13 => Some("&&"),
        14 => Some("||"),
        15 => Some("!"),
        16 => Some("&"),
        17 => Some("|"),
        18 => Some("~"),
        19 => Some("^"),
        20 => Some("<<"),
        21 => Some(">>"),
        22 => Some("+"),
        23 => Some("-"),
        24 => Some("*"),
        25 => Some("**"),
        26 => Some("/"),
        27 => Some("%"),
        28 => Some("="),
        29 => Some("+="),
        30 => Some("-="),
        31 => Some("*="),
        32 => Some("**="),
        33 => Some("/="),
        34 => Some("%="),
        35 => Some("<<="),
        36 => Some(">>="),
        37 => Some("&="),
        38 => Some("|="),
        39 => Some("^="),
        40 => Some("if"),
        41 => Some("elif"),
        42 => Some("else"),
        43 => Some("for"),
        44 => Some("while"),
        45 => Some("break"),
        46 => Some("continue"),
        47 => Some("pass"),
        48 => Some("return"),
        49 => Some("match"),
        50 => Some("when"),
        51 => Some("as"),
        52 => Some("assert"),
        53 => Some("await"),
        54 => Some("breakpoint"),
        55 => Some("class"),
        56 => Some("class_name"),
        57 => Some("const"),
        58 => Some("enum"),
        59 => Some("extends"),
        60 => Some("func"),
        61 => Some("in"),
        62 => Some("is"),
        63 => Some("namespace"),
        64 => Some("preload"),
        65 => Some("self"),
        66 => Some("signal"),
        67 => Some("static"),
        68 => Some("super"),
        69 => Some("trait"),
        70 => Some("var"),
        71 => Some("void"),
        72 => Some("yield"),
        73 => Some("["),
        74 => Some("]"),
        75 => Some("{"),
        76 => Some("}"),
        77 => Some("("),
        78 => Some(")"),
        79 => Some(","),
        80 => Some(";"),
        81 => Some("."),
        82 => Some(".."),
        83 => Some("..."),
        84 => Some(":"),
        85 => Some("$"),
        86 => Some("->"),
        87 => Some("_"),
        91 => Some("PI"),
        92 => Some("TAU"),
        93 => Some("INF"),
        94 => Some("NAN"),
        96 => Some("`"),
        97 => Some("?"),
        _ => None,
    }
}

/// Tokens that want a space before them (binary operators, keywords after expressions).
#[must_use]
pub const fn space_before(kind: u32) -> bool {
    matches!(
        kind,
        4..=9           // < <= > >= == !=
        | 10..=14       // and or not && ||
        | 16 | 17 | 19..=21  // & | ^ << >>
        | 22..=27       // + - * ** / %
        | 28..=39       // = += -= *= **= /= %= <<= >>= &= |= ^=
        | 40 | 42       // if else
        | 51            // as
        | 61 | 62       // in is
        | 86            // ->
    )
}

/// Tokens that want a space after them.
#[must_use]
pub const fn space_after(kind: u32) -> bool {
    matches!(
        kind,
        79 | 80 | 86                // , ; ->
        | 4..=9                      // < <= > >= == !=
        | 10..=15                    // and or not && || !
        | 16 | 17 | 19..=21          // & | ^ << >>
        | 22..=27                    // + - * ** / %
        | 28..=39                    // = += -= *= **= /= %= <<= >>= &= |= ^=
        | 84                         // :
        | 1                          // @ annotations
        | 40..=50                    // if elif else for while break continue pass return match when
        | 51..=55                    // as assert await breakpoint class
        | 56..=60                    // class_name const enum extends func
        | 61..=65                    // in is namespace preload self
        | 66..=70                    // signal static super trait var
        | 71 | 72                    // void yield
    )
}
