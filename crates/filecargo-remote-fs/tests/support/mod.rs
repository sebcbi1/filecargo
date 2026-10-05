#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Helpers shared by the integration test files.

use sha2::{Digest, Sha256};

/// Deterministic, incompressible-ish bytes so resume bugs show up as hash mismatches.
pub fn sample_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
