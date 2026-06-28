use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{
    data,
    index::PrefixLookupResult,
    multi_index::{EntryIndex, File, Layer, PackIndex, Version},
};

/// Represents an entry within a multi index file, effectively mapping object [`IDs`][gix_hash::ObjectId] to pack data
/// files and the offset within.
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entry {
    /// The ID of the object.
    pub oid: gix_hash::ObjectId,
    /// The offset to the object's header in the pack data file.
    pub pack_offset: data::Offset,
    /// The index of the pack matching the position in [`File::index_names()`].
    pub pack_index: PackIndex,
}

/// Access methods
impl<T> File<T>
where
    T: crate::FileData,
{
    /// Returns the version of the multi-index file.
    pub fn version(&self) -> Version {
        self.version
    }
    /// Returns the path from which the multi-index file was loaded.
    ///
    /// For multi-pack index chains, this is the path of the `multi-pack-index-chain` file.
    ///
    /// Note that it might have changed in the mean time, or might have been removed as well.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Returns the directory containing the index- and pack-files referred to by [`index_names()`](File::index_names()).
    ///
    /// Note that for multi-pack index chains, this differs from the parent directory of [`path()`](File::path()),
    /// as the chain itself is stored in a sub-directory of the pack directory.
    pub fn pack_dir(&self) -> &Path {
        &self.pack_dir
    }
    /// Returns the amount of indices stored in this multi-index file, and returned as one past the highest known index.
    ///
    /// For multi-pack index chains, this is the sum over all layers.
    pub fn num_indices(&self) -> PackIndex {
        self.num_indices
    }
    /// Returns the total amount of objects available for lookup, and returned as one past the highest known entry index
    ///
    /// For multi-pack index chains, this is the sum over all layers.
    pub fn num_objects(&self) -> EntryIndex {
        self.num_objects
    }
    /// Returns the amount of layers of the multi-pack index chain, or 1 if this is a standalone multi-pack index file.
    pub fn num_layers(&self) -> usize {
        self.layers.len()
    }
    /// Returns the kind of hash function used for object ids available in this index.
    pub fn object_hash(&self) -> gix_hash::Kind {
        self.object_hash
    }
    /// Returns the checksum over the entire content of the file (excluding the checksum itself).
    ///
    /// For multi-pack index chains, this is the checksum of the most recent layer, which identifies
    /// the chain as it is also the last entry in the chain file.
    ///
    /// It can be used to validate it didn't change after creation.
    pub fn checksum(&self) -> gix_hash::ObjectId {
        self.tip_layer().checksum(self.hash_len)
    }
    /// Return all names of index files (`*.idx`) whose objects we contain, relative to [`pack_dir()`](File::pack_dir()).
    ///
    /// The corresponding pack can be found by replacing the `.idx` extension with `.pack`.
    pub fn index_names(&self) -> impl Iterator<Item = &PathBuf> + '_ {
        self.layers.iter().flat_map(|layer| layer.index_names.iter())
    }
}

impl<T> File<T>
where
    T: crate::FileData,
{
    fn tip_layer(&self) -> &Layer<T> {
        self.layers.last().expect("at least one layer")
    }

    /// Return the layer containing the object at the given global `index`, along with the index local to it.
    fn layer_and_local_index(&self, index: EntryIndex) -> (&Layer<T>, EntryIndex) {
        let layer_index = self.layers.partition_point(|layer| layer.objects_in_base <= index) - 1;
        let layer = &self.layers[layer_index];
        (layer, index - layer.objects_in_base)
    }

    pub(crate) fn oid_at_layer_index<'a>(&self, layer: &'a Layer<T>, local_index: EntryIndex) -> &'a gix_hash::oid {
        debug_assert!(local_index < layer.num_objects, "layer-local index out of bounds");
        let start = layer.lookup_ofs + local_index as usize * self.hash_len;
        gix_hash::oid::from_bytes_unchecked(&layer.data[start..][..self.hash_len])
    }

    /// Return the object id at the given `index`, which ranges from 0 to [File::num_objects()].
    pub fn oid_at_index(&self, index: EntryIndex) -> &gix_hash::oid {
        debug_assert!(index < self.num_objects, "index out of bounds");
        let (layer, local_index) = self.layer_and_local_index(index);
        self.oid_at_layer_index(layer, local_index)
    }

    /// Given a `prefix`, find an object that matches it uniquely within this index and return `Some(Ok(entry_index))`.
    /// If there is more than one object matching the object `Some(Err(())` is returned.
    ///
    /// Finally, if no object matches the index, the return value is `None`.
    ///
    /// Pass `candidates` to obtain the ranges of entry-indices matching `prefix`, with the same return value as
    /// one would have received if it remained `None`. It will be empty if no object matched the `prefix`.
    /// There is one range per layer of the multi-pack index chain that contains at least one match,
    /// thus standalone multi-pack index files yield at most one range.
    ///
    // NOTE: pretty much the same things as in `index::File::lookup`, change things there
    //       as well.
    pub fn lookup_prefix(
        &self,
        prefix: gix_hash::Prefix,
        mut candidates: Option<&mut Vec<Range<EntryIndex>>>,
    ) -> Option<PrefixLookupResult> {
        if let Some(candidates) = candidates.as_deref_mut() {
            candidates.clear();
        }
        let mut result = None;
        // Probe layers from the most recent to the base, like git's `bsearch_midx()`.
        for layer in self.layers.iter().rev() {
            let mut layer_candidates = 0..0;
            let layer_result = crate::index::access::lookup_prefix(
                prefix,
                if candidates.is_some() {
                    Some(&mut layer_candidates)
                } else {
                    None
                },
                &layer.fan,
                &|local_index| self.oid_at_layer_index(layer, local_index),
                layer.num_objects,
            );
            if let Some(candidates) = candidates.as_deref_mut() {
                if !layer_candidates.is_empty() {
                    candidates.push(
                        layer_candidates.start + layer.objects_in_base..layer_candidates.end + layer.objects_in_base,
                    );
                }
            }
            let Some(layer_result) = layer_result else { continue };
            let layer_result = layer_result.map(|local_index| local_index + layer.objects_in_base);
            result = Some(match (result, layer_result) {
                (None, layer_result) => layer_result,
                // The same object may exist in multiple layers, in which case the most recent one wins,
                // but matches on different objects are ambiguous.
                (Some(Ok(previous)), Ok(index)) if self.oid_at_index(previous) == self.oid_at_index(index) => {
                    Ok(previous)
                }
                (Some(_), _) => Err(()),
            });
        }
        result
    }

    /// Find the index ranging from 0 to [File::num_objects()] that belongs to data associated with `id`, or `None` if it wasn't found.
    ///
    /// Use this index for finding additional information via [`File::pack_id_and_pack_offset_at_index()`].
    pub fn lookup(&self, id: impl AsRef<gix_hash::oid>) -> Option<EntryIndex> {
        let id = id.as_ref();
        // Probe layers from the most recent to the base, like git's `bsearch_midx()`.
        self.layers.iter().rev().find_map(|layer| {
            crate::index::access::lookup(id, &layer.fan, &|local_index| {
                self.oid_at_layer_index(layer, local_index)
            })
            .map(|local_index| local_index + layer.objects_in_base)
        })
    }

    pub(crate) fn pack_id_and_pack_offset_at_layer_index(
        &self,
        layer: &Layer<T>,
        local_index: EntryIndex,
    ) -> (PackIndex, data::Offset) {
        const OFFSET_ENTRY_SIZE: usize = 4 + 4;
        const HIGH_BIT: u32 = 1 << 31;

        let start = layer.offsets_ofs + local_index as usize * OFFSET_ENTRY_SIZE;
        let pack_index = crate::read_u32(&layer.data[start..][..4]);
        let ofs32 = crate::read_u32(&layer.data[start + 4..][..4]);
        let pack_offset = if (ofs32 & HIGH_BIT) == HIGH_BIT {
            if let Some(offsets_64) = &layer.large_offsets {
                let from = offsets_64.start + (ofs32 ^ HIGH_BIT) as usize * 8;
                crate::read_u64(&layer.data[from..][..8])
            } else {
                u64::from(ofs32)
            }
        } else {
            u64::from(ofs32)
        };
        (pack_index + layer.indices_in_base, pack_offset)
    }

    pub(crate) fn checked_pack_id_and_pack_offset_at_layer_index(
        &self,
        layer: &Layer<T>,
        local_index: EntryIndex,
        index: EntryIndex,
    ) -> Result<(PackIndex, data::Offset), crate::multi_index::verify::integrity::Error> {
        const OFFSET_ENTRY_SIZE: usize = 4 + 4;
        const HIGH_BIT: u32 = 1 << 31;

        let start = layer.offsets_ofs + local_index as usize * OFFSET_ENTRY_SIZE;
        let pack_index = crate::read_u32(&layer.data[start..][..4]);
        if pack_index >= layer.num_indices {
            return Err(crate::multi_index::verify::integrity::Error::PackIndexOutOfBounds {
                index,
                pack_index,
                num_indices: layer.num_indices,
            });
        }

        let ofs32 = crate::read_u32(&layer.data[start + 4..][..4]);
        let pack_offset = if (ofs32 & HIGH_BIT) == HIGH_BIT {
            if let Some(offsets_64) = &layer.large_offsets {
                let large_offset_index = ofs32 ^ HIGH_BIT;
                let num_large_offsets = offsets_64.len() / 8;
                if large_offset_index as usize >= num_large_offsets {
                    return Err(crate::multi_index::verify::integrity::Error::LargeOffsetOutOfBounds {
                        index,
                        large_offset_index,
                        num_large_offsets,
                    });
                }
                let from = offsets_64.start + large_offset_index as usize * 8;
                crate::read_u64(&layer.data[from..][..8])
            } else {
                u64::from(ofs32)
            }
        } else {
            u64::from(ofs32)
        };
        Ok((pack_index + layer.indices_in_base, pack_offset))
    }

    /// Given the `index` ranging from 0 to [File::num_objects()], return the pack index and its absolute offset into the pack.
    ///
    /// The pack-index refers to an entry in the [`index_names`][File::index_names()] list, from which the pack can be derived.
    pub fn pack_id_and_pack_offset_at_index(&self, index: EntryIndex) -> (PackIndex, data::Offset) {
        let (layer, local_index) = self.layer_and_local_index(index);
        self.pack_id_and_pack_offset_at_layer_index(layer, local_index)
    }

    /// Return an iterator over all entries within this file.
    ///
    /// Note that for multi-pack index chains, entries are sorted by object id within each layer,
    /// but not across layers.
    pub fn iter(&self) -> impl Iterator<Item = Entry> + '_ {
        self.layers.iter().flat_map(move |layer| {
            (0..layer.num_objects).map(move |local_index| {
                let (pack_index, pack_offset) = self.pack_id_and_pack_offset_at_layer_index(layer, local_index);
                Entry {
                    oid: self.oid_at_layer_index(layer, local_index).to_owned(),
                    pack_offset,
                    pack_index,
                }
            })
        })
    }
}
