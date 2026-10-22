//! In-memory byte deltas. Each history edge stores only the direction needed
//! next: backwards before the cursor, forwards after it. Applying an edge
//! builds its inverse before publishing the restored document.
use std::{collections::HashMap, hash::Hasher, ops::Range};

pub struct Patch {
    parts: Vec<Part>,
    length: usize,
}

enum Part {
    Copy(Range<usize>),
    Insert(Vec<u8>),
}

impl Patch {
    pub fn between(source: &[u8], target: &[u8]) -> Self {
        let mut index: HashMap<u64, Vec<Range<usize>>> = HashMap::new();
        for range in chunks(source) {
            index
                .entry(hash(&source[range.clone()]))
                .or_default()
                .push(range);
        }
        let mut parts = Vec::new();
        for range in chunks(target) {
            let bytes = &target[range];
            // Hashes locate candidates only. Exact comparison makes collisions
            // harmless, including repeated chunks in image-heavy PDFs.
            let matching = index.get(&hash(bytes)).and_then(|ranges| {
                ranges
                    .iter()
                    .find(|range| &source[(*range).clone()] == bytes)
            });
            match matching {
                Some(range) => match parts.last_mut() {
                    Some(Part::Copy(previous)) if previous.end == range.start => {
                        previous.end = range.end;
                    }
                    _ => parts.push(Part::Copy(range.clone())),
                },
                None => match parts.last_mut() {
                    Some(Part::Insert(previous)) => previous.extend_from_slice(bytes),
                    _ => parts.push(Part::Insert(bytes.to_vec())),
                },
            }
        }
        Self {
            parts,
            length: target.len(),
        }
    }

    pub fn apply(&self, source: &[u8]) -> Vec<u8> {
        let mut target = Vec::with_capacity(self.length);
        for part in &self.parts {
            match part {
                Part::Copy(range) => target.extend_from_slice(&source[range.clone()]),
                Part::Insert(bytes) => target.extend_from_slice(bytes),
            }
        }
        target
    }

    pub fn retained_bytes(&self) -> usize {
        self.parts.capacity() * std::mem::size_of::<Part>()
            + self
                .parts
                .iter()
                .map(|part| match part {
                    Part::Copy(_) => 0,
                    Part::Insert(bytes) => bytes.capacity(),
                })
                .sum::<usize>()
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

fn chunks(bytes: &[u8]) -> impl Iterator<Item = Range<usize>> + '_ {
    // Content-defined boundaries resynchronize after inserted/deleted bytes.
    // Shift/add forgets bytes older than 64 positions; a mixed byte table
    // avoids the strong bias of hashing raw byte values directly.
    const GEAR: [u64; 256] = {
        let mut table = [0; 256];
        let mut i = 0;
        while i < 256 {
            let mut n = (i as u64).wrapping_add(0x9e3779b97f4a7c15);
            n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
            table[i] = n ^ (n >> 31);
            i += 1;
        }
        table
    };
    let mut start = 0;
    std::iter::from_fn(move || {
        if start == bytes.len() {
            return None;
        }
        let mut end = (start + 4096).min(bytes.len());
        let limit = (start + 65536).min(bytes.len());
        let mut fingerprint = 0u64;
        while end < limit {
            fingerprint = (fingerprint << 1).wrapping_add(GEAR[bytes[end] as usize]);
            end += 1;
            if fingerprint & 8191 == 0 {
                break;
            }
        }
        let range = start..end;
        start = end;
        Some(range)
    })
}
