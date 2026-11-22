//! Map synchronisation (lot 5.1): the local map cache and the handler of
//! `MapChange` and of the map download on channel 3.
//!
//! The cache key is the map fingerprint, never a file name: a file found in
//! a map folder or in the cache is used only if the fingerprint of its bytes
//! equals the announced one. A map the client lacks is requested from the
//! server ([`faset_server::transfer::request_message`], sent on the event
//! channel) and arrives as compressed chunks on channel 3; the client
//! inflates it (bounded), recomputes the fingerprint and rejects a
//! mismatch. The handler is transport-agnostic: the caller sends the
//! returned messages and feeds the chunks it receives.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use faset_protocol::Message;
use faset_server::map_fingerprint;
use faset_server::transfer::{
    decompress_map, request_message, TransferError, MAX_MAP_TRANSFER_BYTES, MAX_TRANSFER_CHUNKS,
};

/// Why a downloaded map was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MapSyncError {
    /// The map hashes to another fingerprint than the one announced
    /// (tampered or wrong payload).
    #[error("map fingerprint mismatch: expected {expected:016x}, got {actual:016x}")]
    FingerprintMismatch { expected: u64, actual: u64 },
    /// The compressed payload is invalid or inflates past the limit.
    #[error("map payload rejected: {0}")]
    Payload(#[from] TransferError),
}

/// What happened to the map being fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapEvent {
    /// Download progress, in chunks.
    Progress {
        fingerprint: u64,
        received: u16,
        total: u16,
    },
    /// The map file is available (from the cache, a folder or the server);
    /// load it, then report ready with [`MapSync::ready_message`].
    Ready { fingerprint: u64, bytes: Vec<u8> },
    /// The download was refused; nothing was cached.
    Failed {
        fingerprint: u64,
        error: MapSyncError,
    },
}

/// Maps the client already has: memory, a cache folder and read-only map
/// folders, all looked up by fingerprint.
#[derive(Debug, Default)]
pub struct MapCache {
    memory: HashMap<u64, Vec<u8>>,
    cache_dir: Option<PathBuf>,
    folders: Vec<PathBuf>,
}

impl MapCache {
    /// A cache in memory only.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Also keeps downloaded maps in `dir` (`<fingerprint>.map`).
    #[must_use]
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// Also looks in the map folder `dir` (any file whose fingerprint
    /// matches, whatever its name).
    #[must_use]
    pub fn with_folder(mut self, dir: impl Into<PathBuf>) -> Self {
        self.folders.push(dir.into());
        self
    }

    fn cache_path(&self, fingerprint: u64) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|d| d.join(format!("{fingerprint:016x}.map")))
    }

    /// The bytes of the map with this fingerprint, if present anywhere.
    /// Files are re-hashed: a corrupt or swapped file is ignored.
    #[must_use]
    pub fn get(&mut self, fingerprint: u64) -> Option<Vec<u8>> {
        if let Some(b) = self.memory.get(&fingerprint) {
            return Some(b.clone());
        }
        let from_file = self
            .cache_path(fingerprint)
            .and_then(|p| read_map(&p))
            .filter(|b| map_fingerprint(b) == fingerprint)
            .or_else(|| {
                self.folders
                    .iter()
                    .find_map(|d| find_in_folder(d, fingerprint))
            })?;
        self.memory.insert(fingerprint, from_file.clone());
        Some(from_file)
    }

    /// Stores a verified map.
    pub fn insert(&mut self, fingerprint: u64, bytes: Vec<u8>) {
        if let Some(p) = self.cache_path(fingerprint) {
            let stored = p
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&p, &bytes));
            if let Err(e) = stored {
                tracing::warn!(path = %p.display(), error = %e, "cannot write the map cache");
            }
        }
        self.memory.insert(fingerprint, bytes);
    }
}

fn read_map(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MAP_TRANSFER_BYTES as u64 {
        return None;
    }
    std::fs::read(path).ok()
}

fn find_in_folder(dir: &Path, fingerprint: u64) -> Option<Vec<u8>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|p| read_map(p))
        .find(|b| map_fingerprint(b) == fingerprint)
}

#[derive(Debug)]
struct Download {
    fingerprint: u64,
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
    received: u16,
}

/// The `MapChange` handler and map download state of one client.
#[derive(Debug, Default)]
pub struct MapSync {
    cache: MapCache,
    download: Option<Download>,
    events: VecDeque<MapEvent>,
}

impl MapSync {
    /// A handler over `cache`.
    #[must_use]
    pub fn new(cache: MapCache) -> Self {
        Self {
            cache,
            download: None,
            events: VecDeque::new(),
        }
    }

    /// The cache.
    #[must_use]
    pub fn cache(&self) -> &MapCache {
        &self.cache
    }

    /// The server announced the map `fingerprint` (a `MapChange`, or the
    /// handshake). A map found locally yields [`MapEvent::Ready`] at once
    /// and returns `None`; otherwise returns the request to send to the
    /// server on the event channel.
    pub fn on_map_change(&mut self, fingerprint: u64) -> Option<Message> {
        self.download = None;
        if let Some(bytes) = self.cache.get(fingerprint) {
            self.events
                .push_back(MapEvent::Ready { fingerprint, bytes });
            return None;
        }
        self.download = Some(Download {
            fingerprint,
            total: 0,
            chunks: Vec::new(),
            received: 0,
        });
        Some(request_message(fingerprint))
    }

    /// The message that tells the server the map is loaded.
    #[must_use]
    pub fn ready_message() -> Message {
        Message::Ready
    }

    /// Feeds one message received on channel 3. Anything else, a chunk with
    /// no download in progress, or an inconsistent chunk is ignored.
    pub fn on_chunk(&mut self, msg: &Message) {
        let Message::MapChunk { index, total, data } = msg else {
            return;
        };
        let Some(d) = self.download.as_mut() else {
            return;
        };
        let (index, total) = (usize::from(*index), usize::from(*total));
        if total == 0 || total > MAX_TRANSFER_CHUNKS || index >= total {
            return;
        }
        if d.chunks.is_empty() {
            d.total = u16::try_from(total).unwrap_or(0);
            d.chunks = vec![None; total];
        } else if d.chunks.len() != total {
            return;
        }
        if d.chunks[index].is_none() {
            d.chunks[index] = Some(data.clone());
            d.received += 1;
        }
        self.events.push_back(MapEvent::Progress {
            fingerprint: d.fingerprint,
            received: d.received,
            total: d.total,
        });
        if d.received < d.total {
            return;
        }
        let Some(done) = self.download.take() else {
            return;
        };
        let compressed: Vec<u8> = done.chunks.into_iter().flatten().flatten().collect();
        let event = match decompress_map(&compressed) {
            Err(e) => MapEvent::Failed {
                fingerprint: done.fingerprint,
                error: e.into(),
            },
            Ok(bytes) => {
                let actual = map_fingerprint(&bytes);
                if actual == done.fingerprint {
                    self.cache.insert(actual, bytes.clone());
                    MapEvent::Ready {
                        fingerprint: actual,
                        bytes,
                    }
                } else {
                    MapEvent::Failed {
                        fingerprint: done.fingerprint,
                        error: MapSyncError::FingerprintMismatch {
                            expected: done.fingerprint,
                            actual,
                        },
                    }
                }
            }
        };
        self.events.push_back(event);
    }

    /// Next event, oldest first.
    pub fn poll_event(&mut self) -> Option<MapEvent> {
        self.events.pop_front()
    }

    /// Whether a download is in progress.
    #[must_use]
    pub fn downloading(&self) -> bool {
        self.download.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_server::transfer::{chunk_messages, compress_map};

    fn file() -> Vec<u8> {
        (0..40_000u32)
            .flat_map(|i| (i.wrapping_mul(2_654_435_761) >> 20).to_le_bytes())
            .collect()
    }

    fn feed(sync: &mut MapSync, chunks: &[Message]) {
        for c in chunks {
            sync.on_chunk(c);
        }
    }

    fn drain(sync: &mut MapSync) -> Vec<MapEvent> {
        std::iter::from_fn(|| sync.poll_event()).collect()
    }

    #[test]
    fn unknown_map_is_requested_downloaded_verified_and_cached() {
        let bytes = file();
        let fp = map_fingerprint(&bytes);
        let chunks = chunk_messages(&compress_map(&bytes).unwrap()).unwrap();
        assert!(chunks.len() > 3);
        let mut sync = MapSync::default();
        let req = sync.on_map_change(fp).expect("a request");
        assert_eq!(faset_server::transfer::parse_request(&req), Some(fp));
        assert!(sync.downloading());
        feed(&mut sync, &chunks);
        let events = drain(&mut sync);
        let progress = events
            .iter()
            .filter(|e| matches!(e, MapEvent::Progress { .. }))
            .count();
        assert_eq!(progress, chunks.len());
        assert_eq!(
            events.last(),
            Some(&MapEvent::Ready {
                fingerprint: fp,
                bytes: bytes.clone()
            })
        );
        // Second time: from the cache, no request.
        assert!(sync.on_map_change(fp).is_none());
        assert!(matches!(sync.poll_event(), Some(MapEvent::Ready { .. })));
    }

    #[test]
    fn a_tampered_payload_is_rejected_and_not_cached() {
        let bytes = file();
        let fp = map_fingerprint(&bytes);
        // Valid DEFLATE of other bytes: only the fingerprint check catches it.
        let mut evil = bytes.clone();
        evil[1234] ^= 1;
        let chunks = chunk_messages(&compress_map(&evil).unwrap()).unwrap();
        let mut sync = MapSync::default();
        sync.on_map_change(fp);
        feed(&mut sync, &chunks);
        let last = drain(&mut sync).pop().unwrap();
        assert!(
            matches!(
                last,
                MapEvent::Failed {
                    error: MapSyncError::FingerprintMismatch { .. },
                    ..
                }
            ),
            "{last:?}"
        );
        assert!(sync.cache.get(fp).is_none());

        // Corrupt compressed bytes.
        let mut packed = compress_map(&bytes).unwrap();
        let mid = packed.len() / 2;
        packed[mid] ^= 0xFF;
        packed.truncate(packed.len() - 3);
        sync.on_map_change(fp);
        feed(&mut sync, &chunk_messages(&packed).unwrap());
        let last = drain(&mut sync).pop().unwrap();
        assert!(matches!(last, MapEvent::Failed { .. }), "{last:?}");
        assert!(sync.cache.get(fp).is_none());
    }

    #[test]
    fn chunks_in_any_order_and_junk_are_handled() {
        let bytes = file();
        let fp = map_fingerprint(&bytes);
        let mut chunks = chunk_messages(&compress_map(&bytes).unwrap()).unwrap();
        let mut sync = MapSync::default();
        // No download yet: ignored.
        feed(&mut sync, &chunks);
        assert!(drain(&mut sync).is_empty());
        sync.on_map_change(fp);
        chunks.reverse();
        let mut junk = chunks.clone();
        junk.push(Message::MapChunk {
            index: 9,
            total: 2,
            data: vec![1],
        });
        junk.push(Message::Ready);
        feed(&mut sync, &junk);
        assert!(matches!(
            drain(&mut sync).pop(),
            Some(MapEvent::Ready { .. })
        ));
    }

    #[test]
    fn cache_dir_and_folders_are_looked_up_by_fingerprint() {
        let root = std::env::temp_dir().join(format!("faset-mapsync-{}", std::process::id()));
        let (folder, cache) = (root.join("maps"), root.join("cache"));
        std::fs::create_dir_all(&folder).unwrap();
        let bytes = file();
        let fp = map_fingerprint(&bytes);
        // The folder file has an unrelated name; a decoy has other bytes.
        std::fs::write(folder.join("whatever.txt"), &bytes).unwrap();
        std::fs::write(folder.join("a-decoy.ron"), b"other").unwrap();
        let mut c = MapCache::new().with_folder(&folder).with_cache_dir(&cache);
        assert_eq!(c.get(fp), Some(bytes.clone()));
        assert_eq!(c.get(fp ^ 1), None);
        // A downloaded map lands in the cache dir and survives a restart.
        let other = vec![7u8; 100];
        let fp2 = map_fingerprint(&other);
        c.insert(fp2, other.clone());
        assert_eq!(MapCache::new().with_cache_dir(&cache).get(fp2), Some(other));
        // A swapped cache file is ignored.
        std::fs::write(cache.join(format!("{fp2:016x}.map")), b"swapped").unwrap();
        assert_eq!(MapCache::new().with_cache_dir(&cache).get(fp2), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
