//! Map transfer (lot 5.1): compression and chunking of a map file for
//! channel 3, and the matching decompression.
//!
//! Decision: the compressor is DEFLATE (`miniz_oxide`, pure Rust, already in
//! the dependency tree), level 9: map files are RON text and shrink a lot.
//! The transfer is pull-based: a client that lacks the map named by a
//! `MapChange` (or by the handshake) sends a request on the event channel
//! (channel 3 is server to client only), and the server answers on channel 3
//! with [`MAX_CHUNK_BYTES`]-byte [`Message::MapChunk`]s.

use faset_protocol::message::MAX_CHUNK_BYTES;
use faset_protocol::Message;

/// Largest downloadable map file, uncompressed (section 6: 256 KB, the same
/// limit as `faset_mapfmt::MAX_MAP_BYTES`).
pub const MAX_MAP_TRANSFER_BYTES: usize = 262_144;
/// Most chunks of one transfer (the compressed size of a map is below its
/// size plus a few bytes per 64 KB block).
pub const MAX_TRANSFER_CHUNKS: usize = MAX_MAP_TRANSFER_BYTES / MAX_CHUNK_BYTES + 8;

const COMPRESSION_LEVEL: u8 = 9;

/// Why a map could not be compressed or decompressed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    /// The map file is larger than [`MAX_MAP_TRANSFER_BYTES`].
    #[error("map is {0} bytes, limit is {MAX_MAP_TRANSFER_BYTES}")]
    TooLarge(usize),
    /// The payload is not valid DEFLATE or inflates past the limit.
    #[error("corrupt or oversized compressed map")]
    Corrupt,
    /// The chunks do not describe one transfer (bad index, total, size).
    #[error("inconsistent map chunk")]
    BadChunk,
}

/// The request a client sends to ask for the map of `fingerprint`: a
/// `MapChunk` with index 0, total 0 and the fingerprint (little endian) as
/// data (decision: the protocol has no dedicated request message).
#[must_use]
pub fn request_message(fingerprint: u64) -> Message {
    Message::MapChunk {
        index: 0,
        total: 0,
        data: fingerprint.to_le_bytes().to_vec(),
    }
}

/// The fingerprint asked for by a request built by [`request_message`].
#[must_use]
pub fn parse_request(msg: &Message) -> Option<u64> {
    match msg {
        Message::MapChunk { total: 0, data, .. } => {
            Some(u64::from_le_bytes(data.as_slice().try_into().ok()?))
        }
        _ => None,
    }
}

/// Compresses a map file.
///
/// # Errors
/// [`TransferError::TooLarge`] above [`MAX_MAP_TRANSFER_BYTES`].
pub fn compress_map(file: &[u8]) -> Result<Vec<u8>, TransferError> {
    if file.len() > MAX_MAP_TRANSFER_BYTES {
        return Err(TransferError::TooLarge(file.len()));
    }
    Ok(miniz_oxide::deflate::compress_to_vec(
        file,
        COMPRESSION_LEVEL,
    ))
}

/// Inflates a compressed map, refusing more than [`MAX_MAP_TRANSFER_BYTES`]
/// (a decompression bomb).
///
/// # Errors
/// [`TransferError::Corrupt`].
pub fn decompress_map(compressed: &[u8]) -> Result<Vec<u8>, TransferError> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(compressed, MAX_MAP_TRANSFER_BYTES)
        .map_err(|_| TransferError::Corrupt)
}

/// Splits a compressed map into the chunks of one transfer.
///
/// # Errors
/// [`TransferError::TooLarge`] if it needs more than [`MAX_TRANSFER_CHUNKS`].
pub fn chunk_messages(compressed: &[u8]) -> Result<Vec<Message>, TransferError> {
    let total = compressed.len().div_ceil(MAX_CHUNK_BYTES).max(1);
    if total > MAX_TRANSFER_CHUNKS {
        return Err(TransferError::TooLarge(compressed.len()));
    }
    let mut parts: Vec<&[u8]> = compressed.chunks(MAX_CHUNK_BYTES).collect();
    if parts.is_empty() {
        parts.push(&[]);
    }
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(i, data)| Message::MapChunk {
            index: i as u16,
            total: total as u16,
            data: data.to_vec(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_in_chunks() {
        let file: Vec<u8> = (0..60_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let packed = compress_map(&file).unwrap();
        let chunks = chunk_messages(&packed).unwrap();
        assert!(chunks.iter().all(|m| m.encode().unwrap().len() <= 1200));
        assert_eq!(decompress_map(&packed).unwrap(), file);
    }

    #[test]
    fn request_round_trip_and_chunks_are_not_requests() {
        let m = request_message(0xDEAD_BEEF_0123_4567);
        assert_eq!(parse_request(&m), Some(0xDEAD_BEEF_0123_4567));
        let c = chunk_messages(&compress_map(b"abc").unwrap()).unwrap();
        assert_eq!(parse_request(&c[0]), None);
    }

    #[test]
    fn bombs_and_junk_are_refused() {
        let bomb = compress_map(&vec![0u8; MAX_MAP_TRANSFER_BYTES]).unwrap();
        assert!(decompress_map(&bomb).is_ok());
        let too_big =
            miniz_oxide::deflate::compress_to_vec(&vec![0u8; MAX_MAP_TRANSFER_BYTES + 1], 6);
        assert_eq!(decompress_map(&too_big), Err(TransferError::Corrupt));
        assert_eq!(decompress_map(b"not deflate"), Err(TransferError::Corrupt));
        assert!(compress_map(&vec![0u8; MAX_MAP_TRANSFER_BYTES + 1]).is_err());
    }
}
