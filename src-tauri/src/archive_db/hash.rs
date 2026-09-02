//! Content hashing for `source_record.content_hash` -- provenance only,
//! not a security boundary.

/// Hex-encoded BLAKE3 digest of a single ingested line's raw bytes.
pub fn hash_line(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_line_is_deterministic() {
        assert_eq!(hash_line(b"hello"), hash_line(b"hello"));
    }

    #[test]
    fn hash_line_differs_for_different_input() {
        assert_ne!(hash_line(b"hello"), hash_line(b"world"));
    }

    #[test]
    fn hash_line_is_64_hex_chars() {
        let digest = hash_line(b"some jsonl line content");
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
