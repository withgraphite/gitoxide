use std::{ops::Range, path::PathBuf};

use crate::MMap;

/// Known multi-index file versions
#[derive(Default, PartialEq, Eq, Ord, PartialOrd, Debug, Hash, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)]
pub enum Version {
    #[default]
    V1 = 1,
    V2 = 2,
}

/// An index into the names yielded by [`File::index_names()`], and by implication, their pack files.
pub type PackIndex = u32;

/// The type for referring to indices of an entry within the index file.
pub type EntryIndex = u32;

/// A representation of an index file for multiple packs at the same time, typically stored in a file
/// named 'multi-pack-index'.
///
/// It can also represent an *incremental* multi-pack index, i.e. a chain of multi-pack index files
/// as stored in the `multi-pack-index.d` directory and opened with [`File::at_chain()`].
/// In that case, each file in the chain is a layer, and object positions as well as pack ids
/// are global, that is the concatenation of the per-layer values in chain order, base layer first.
/// A standalone multi-pack index is simply a chain with a single layer.
pub struct File<T = MMap> {
    /// The path the file was read from, or the path of the `multi-pack-index-chain` file for chains.
    path: std::path::PathBuf,
    /// The directory containing the pack and index files named by the layers.
    pack_dir: PathBuf,
    version: Version,
    hash_len: usize,
    object_hash: gix_hash::Kind,
    /// The total amount of pack files summed over all layers.
    num_indices: u32,
    /// The total amount of objects summed over all layers.
    num_objects: u32,
    /// All layers of the (possibly chained) multi-pack index, base layer first. There always is at least one.
    layers: Vec<Layer<T>>,
}

/// A single multi-pack index file, either standalone or as part of a chain of multiple such files.
pub(crate) struct Layer<T> {
    data: T,
    path: PathBuf,
    version: Version,
    /// The amount of base multi-pack index files as stated in this layer's header. Git currently always writes 0 here.
    num_base_files: u8,
    /// The amount of pack files contained within this layer.
    num_indices: u32,
    /// The names of all contained indices, in the order used by entries in this layer.
    index_names: Vec<PathBuf>,
    /// The amount of objects contained within this layer.
    num_objects: u32,
    /// The sum of `num_indices` of all prior layers, or 0 for the base layer.
    indices_in_base: u32,
    /// The sum of `num_objects` of all prior layers, or 0 for the base layer.
    objects_in_base: u32,
    fan: [u32; 256],
    /// The checksum of this layer's base layer as stated in its optional `BASE` chunk.
    /// Note that git currently never writes this chunk - its id is merely reserved.
    base_checksum: Option<gix_hash::ObjectId>,
    lookup_ofs: usize,
    offsets_ofs: usize,
    large_offsets: Option<Range<usize>>,
}

impl<T> Layer<T>
where
    T: crate::FileData,
{
    /// The checksum over the entire content of this layer's file, as stored in its trailer.
    pub(crate) fn checksum(&self, hash_len: usize) -> gix_hash::ObjectId {
        gix_hash::ObjectId::from_bytes_or_panic(&self.data[self.data.len() - hash_len..])
    }
}

/// Errors returned when opening either a standalone multi-pack index or an incremental chain.
pub mod open {
    /// The error returned by [`File::at_path()`](super::File::at_path()).
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error(transparent)]
        Standalone(#[from] super::init::Error),
        #[error(transparent)]
        Chain(#[from] super::chain::Error),
    }
}

///
pub mod write;
pub use write::function::write_from_index_paths;

///
mod access;

///
pub mod chain;

///
pub mod verify;

///
pub mod chunk;

///
pub mod init;
