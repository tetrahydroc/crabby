//! Full-corpus rewriter test.
//!
//! Iterates every `Scripts/*.gdc` in `RTV.pck`, runs
//! `detokenize -> parse_script -> rewrite_full_script` on each, and asserts:
//!
//! 1. Every script survives the pipeline without error.
//! 2. Per-template tally is printed for visibility.
//! 3. Output is stable across runs, per-script SHA-256 hashes are
//!    compared against a committed snapshot at
//!    `tests/fixtures/full_corpus_snapshot.txt`. Regenerate after an
//!    intentional rewriter change by running with `UPDATE_SNAPSHOT=1`.
//!
//! # Opt-in
//!
//! Gated on `CRABBY_RTV_PCK`. Without it the test exits `ok` so CI on
//! machines that don't have the game files stays green.
//!
//! ```sh
//! CRABBY_RTV_PCK="/mnt/e/.../RTV.pck" \
//!     cargo test -p crabby-rewriter --test full_corpus -- --nocapture
//!
//! # Regenerate the snapshot after an intentional rewriter change:
//! UPDATE_SNAPSHOT=1 CRABBY_RTV_PCK="/mnt/e/.../RTV.pck" \
//!     cargo test -p crabby-rewriter --test full_corpus -- --nocapture
//! ```

// The SHA-256 implementation at the bottom of this file follows the
// FIPS 180-4 pseudocode verbatim, which uses single-character bindings
// (a..h, t1, t2, s0, s1) and direct indexed loops over the 64-word
// message schedule. Clippy's readability lints would make the code
// harder to cross-check against the spec; allow them here.
#![allow(
    clippy::case_sensitive_file_extension_comparisons,
    clippy::many_single_char_names,
    clippy::needless_range_loop
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_parser::parse_script;
use crabby_pck::PckArchive;
use crabby_rewriter::{
    is_additive_script, is_data_intercept_script, is_fast_script, rewrite_full_script,
};

const ENV_PCK: &str = "CRABBY_RTV_PCK";
const ENV_UPDATE: &str = "UPDATE_SNAPSHOT";
const SNAPSHOT_FILE: &str = "full_corpus_snapshot.txt";

#[test]
fn full_corpus_rewrites_cleanly_and_matches_snapshot() {
    let Ok(pck_path) = std::env::var(ENV_PCK) else {
        eprintln!("{ENV_PCK} not set, skipping full-corpus rewriter test.");
        return;
    };
    if pck_path.is_empty() {
        eprintln!("{ENV_PCK} empty, skipping full-corpus rewriter test.");
        return;
    }

    let pck_path = PathBuf::from(pck_path);
    let mut archive = PckArchive::open(&pck_path).expect("open RTV.pck");

    let targets: Vec<_> = archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".gdc"))
        .filter(|e| {
            let n = e.path.trim_start_matches("res://").trim_start_matches('/');
            n.starts_with("Scripts/")
        })
        .cloned()
        .collect();
    assert!(!targets.is_empty(), "no Scripts/*.gdc in PCK");

    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut tally = Tally::default();
    let mut failures: Vec<String> = Vec::new();

    for entry in &targets {
        let filename = script_filename(&entry.path);
        let bytes = archive.read(entry).expect("pck read");
        let source = detokenize(&bytes).expect("detokenize");
        if source.is_empty() {
            tally.zero_byte += 1;
            continue;
        }

        classify(&filename, &source, &mut tally);

        let parsed = match parse_script(&filename, &source) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{filename}: parse failed: {e}"));
                continue;
            }
        };

        let rewritten = match rewrite_full_script(&source, &parsed) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{filename}: rewrite failed: {e}"));
                continue;
            }
        };

        hashes.insert(filename, sha256_hex(rewritten.as_bytes()));
        tally.rewritten += 1;
    }

    eprintln!();
    eprintln!("=== full-corpus rewrite summary ===");
    eprintln!("total scripts:       {}", targets.len());
    eprintln!("rewritten ok:        {}", tally.rewritten);
    eprintln!("zero-byte skipped:   {}", tally.zero_byte);
    eprintln!("classified-additive: {}", tally.additive);
    eprintln!("classified-data:     {}", tally.data_intercept);
    eprintln!("classified-fast:     {}", tally.fast);
    eprintln!(
        "classified-standard: {}",
        tally.rewritten - tally.additive - tally.data_intercept - tally.fast,
    );
    eprintln!();

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("  FAIL {f}");
        }
        panic!("{} script(s) failed pipeline", failures.len());
    }

    // Snapshot diff.
    let snapshot_path = snapshot_path();
    let new_snapshot = render_snapshot(&hashes);

    if std::env::var(ENV_UPDATE).is_ok_and(|v| !v.is_empty()) {
        fs::write(&snapshot_path, &new_snapshot)
            .unwrap_or_else(|e| panic!("write {}: {e}", snapshot_path.display()));
        eprintln!(
            "UPDATE_SNAPSHOT set, wrote {} ({} entries).",
            snapshot_path.display(),
            hashes.len(),
        );
        return;
    }

    let existing = match fs::read_to_string(&snapshot_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            panic!(
                "snapshot {} missing; run with UPDATE_SNAPSHOT=1 to generate",
                snapshot_path.display(),
            );
        }
        Err(e) => panic!("read {}: {e}", snapshot_path.display()),
    };

    if existing != new_snapshot {
        let dump = std::env::temp_dir().join("crabby-full-corpus-actual.txt");
        let _ = fs::write(&dump, &new_snapshot);
        // Find the first differing line for a pinpoint panic message.
        let differing = first_diff(&existing, &new_snapshot);
        panic!(
            "full-corpus snapshot diverged at line {differing:?}.\n\
             actual:    {}\n\
             expected:  {}\n\
             Run with UPDATE_SNAPSHOT=1 to regenerate after an intentional rewriter change.",
            dump.display(),
            snapshot_path.display(),
        );
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Tally {
    rewritten: usize,
    zero_byte: usize,
    additive: usize,
    data_intercept: usize,
    fast: usize,
}

/// Bump the per-template counters for reporting. Mirrors the precedence
/// rules in `crabby_rewriter::pick_template` + per-script classification
/// (additive/data-intercept) so the tally matches what actually ran.
fn classify(filename: &str, _source: &str, tally: &mut Tally) {
    if is_additive_script(filename) {
        tally.additive += 1;
    } else if is_data_intercept_script(filename) {
        tally.data_intercept += 1;
    } else if is_fast_script(filename) {
        tally.fast += 1;
    }
}

fn script_filename(path: &str) -> String {
    let normalized = path.trim_start_matches("res://").trim_start_matches('/');
    let stem = Path::new(normalized)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    format!("{stem}.gd")
}

fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(SNAPSHOT_FILE)
}

/// Emit a deterministic `<filename>\t<hex-sha256>\n` manifest.
fn render_snapshot(hashes: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(hashes.len() * 80);
    out.push_str("# crabby-rewriter full-corpus snapshot - ");
    out.push_str("regenerate with UPDATE_SNAPSHOT=1 after an intentional rewriter change.\n");
    for (name, hash) in hashes {
        out.push_str(name);
        out.push('\t');
        out.push_str(hash);
        out.push('\n');
    }
    out
}

/// First line number (1-based) where `actual` and `expected` differ, or
/// `None` when one is a prefix of the other.
fn first_diff(existing: &str, new: &str) -> Option<usize> {
    for (i, (a, b)) in existing.lines().zip(new.lines()).enumerate() {
        if a != b {
            return Some(i + 1);
        }
    }
    None
}

/// Stdlib-only SHA-256. Kept dependency-free on purpose; this is the
/// only place in the workspace that needs a hash function, pulling in a
/// crate for one call would be overkill.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::hash(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// Inline SHA-256 (FIPS 180-4)
// ---------------------------------------------------------------------------

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bits: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    const INIT: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    fn hash(mut input: &[u8]) -> [u8; 32] {
        let mut s = Self {
            state: Self::INIT,
            buf: [0u8; 64],
            buf_len: 0,
            total_bits: (input.len() as u64) * 8,
        };
        while !input.is_empty() {
            let fill = (64 - s.buf_len).min(input.len());
            s.buf[s.buf_len..s.buf_len + fill].copy_from_slice(&input[..fill]);
            s.buf_len += fill;
            input = &input[fill..];
            if s.buf_len == 64 {
                s.process_block();
                s.buf_len = 0;
            }
        }
        // Padding.
        s.buf[s.buf_len] = 0x80;
        s.buf_len += 1;
        if s.buf_len > 56 {
            for b in &mut s.buf[s.buf_len..] {
                *b = 0;
            }
            s.process_block();
            s.buf_len = 0;
        }
        for b in &mut s.buf[s.buf_len..56] {
            *b = 0;
        }
        s.buf[56..64].copy_from_slice(&s.total_bits.to_be_bytes());
        s.process_block();

        let mut out = [0u8; 32];
        for (i, word) in s.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn process_block(&mut self) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(self.buf[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        let mut f = self.state[5];
        let mut g = self.state[6];
        let mut h = self.state[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

#[cfg(test)]
mod sha256_self_test {
    use super::sha256_hex;

    #[test]
    fn empty() {
        // NIST test vector.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }

    #[test]
    fn longer() {
        // 56 bytes, boundary near the pad-length threshold.
        let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            sha256_hex(msg),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        );
    }
}
