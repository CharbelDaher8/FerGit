//! Hash maps keyed by object ids, with a hasher that doesn't rehash them.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::types::Oid;

/// Hashes an [`Oid`] by its first eight bytes.
///
/// Object ids are SHA-1 hashes, already uniformly distributed, so eight of their bytes make a good
/// hash; running SipHash over all twenty is most of a lookup's cost in loops that touch every
/// commit of a long history. Degrading a map would take many ids sharing those eight bytes, and
/// each such id costs on the order of 2^64 hash computations to find.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        match bytes.first_chunk::<8>() {
            Some(chunk) => self.0 ^= u64::from_le_bytes(*chunk),
            None => {
                for &byte in bytes {
                    self.0 = self.0.rotate_left(8) ^ u64::from(byte);
                }
            }
        }
    }
}

pub(crate) type IdMap<V> = HashMap<Oid, V, BuildHasherDefault<IdHasher>>;
