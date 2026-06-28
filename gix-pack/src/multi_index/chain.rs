//! Support for reading incremental multi-pack indexes, also called multi-pack index chains.
//!
//! Instead of a single `multi-pack-index` file in the pack directory, an incremental multi-pack index
//! is stored as a `multi-pack-index.d` directory containing a `multi-pack-index-chain` file along with
//! one `multi-pack-index-$HASH.midx` file per layer. The chain file lists the trailing checksum of each
//! layer file in chain order, base layer first, one hexadecimal checksum per line.
//!
//! Git writes these with `git multi-pack-index write --incremental`, appending a new layer for packs
//! that aren't yet part of the chain instead of rewriting the entire index.
use std::path::Path;

use crate::multi_index::{File, init};

/// The name of the directory within the pack directory in which a multi-pack index chain is stored.
pub const DIRECTORY: &str = "multi-pack-index.d";

/// The name of the file within [`DIRECTORY`] listing the checksum of each layer of the chain.
pub const CHAIN_FILE: &str = "multi-pack-index-chain";

/// Return the file name of the multi-pack index layer file whose trailing checksum is `id`,
/// for lookup within [`DIRECTORY`].
pub fn layer_file_name(id: &gix_hash::oid) -> String {
    format!("multi-pack-index-{id}.midx")
}

mod error {
    use std::path::PathBuf;

    /// The error returned by [File::at_chain()][super::File::at_chain()].
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error("Could not open multi-pack-index chain at '{path}'")]
        Io { source: std::io::Error, path: PathBuf },
        #[error("The multi-pack-index chain file at '{path}' is larger than the allocation limit of {limit} bytes")]
        ChainFileTooLarge { path: PathBuf, limit: usize },
        #[error("The multi-pack-index chain file at '{path}' contains no layers")]
        Empty { path: PathBuf },
        #[error("Line {line_number} in multi-pack-index chain file '{path}' is not a valid hash")]
        InvalidLine { line_number: usize, path: PathBuf },
        #[error("Could not parse multi-pack-index layer")]
        Layer(#[from] crate::multi_index::init::Error),
        #[error("The layer at '{path}' has checksum {actual}, but the chain file expects {expected}")]
        LayerChecksum {
            actual: gix_hash::ObjectId,
            expected: gix_hash::ObjectId,
            path: PathBuf,
        },
        #[error("The layer at '{path}' uses hash kind {actual}, but the chain file implies {expected}")]
        LayerObjectHash {
            actual: gix_hash::Kind,
            expected: gix_hash::Kind,
            path: PathBuf,
        },
        #[error("The layer at '{path}' claims {actual} base files, but its position in the chain implies {expected}")]
        BaseFileCount { actual: u8, expected: usize, path: PathBuf },
        #[error("The layer at '{path}' expects a base layer with checksum {expected}, but found {actual}")]
        BaseChecksum {
            actual: gix_hash::ObjectId,
            expected: gix_hash::ObjectId,
            path: PathBuf,
        },
        #[error("The base layer at '{path}' must not refer to another base layer with checksum {actual}")]
        UnexpectedBaseChecksum { actual: gix_hash::ObjectId, path: PathBuf },
        #[error("The sum of {what} of all layers in the chain at '{path}' is greater than 2^32-1")]
        CountOverflow { what: &'static str, path: PathBuf },
    }
}

pub use error::Error;

/// Initialization of incremental multi-pack indexes.
impl File<crate::MMap> {
    /// Open the multi-pack index chain whose chain file lives at `chain_path`,
    /// typically `<objects>/pack/multi-pack-index.d/multi-pack-index-chain`,
    /// by memory-mapping each layer file listed in it.
    ///
    /// The hash kind is inferred from the length of the checksums in the chain file and
    /// validated against each layer. Each layer is validated to match the checksum the chain
    /// file states for it, and the optional `BASE` chunk as well as the base-file count of
    /// each layer is checked for consistency with its position in the chain.
    ///
    /// `alloc_limit_bytes` bounds each allocation caused by user-controlled on-disk data, useful for untrusted input.
    /// Use `None` to disable the limit.
    pub fn at_chain(chain_path: impl AsRef<Path>, alloc_limit_bytes: Option<usize>) -> Result<Self, Error> {
        Self::at_chain_inner(chain_path.as_ref(), alloc_limit_bytes)
    }

    /// Open the multi-pack index chain at `chain_path` using the already-read contents of its chain file.
    ///
    /// This allows callers that track changes to use the same chain-file snapshot for change detection and opening.
    pub fn at_chain_with_data(
        chain_path: impl AsRef<Path>,
        chain_data: &[u8],
        alloc_limit_bytes: Option<usize>,
    ) -> Result<Self, Error> {
        Self::at_chain_data_inner(chain_path.as_ref(), chain_data, alloc_limit_bytes)
    }

    fn at_chain_inner(chain_path: &Path, alloc_limit_bytes: Option<usize>) -> Result<Self, Error> {
        let into_io_err = |source| Error::Io {
            source,
            path: chain_path.to_owned(),
        };
        if let Some(limit) = alloc_limit_bytes {
            let chain_len = std::fs::metadata(chain_path).map_err(into_io_err)?.len();
            if chain_len > limit as u64 {
                return Err(Error::ChainFileTooLarge {
                    path: chain_path.to_owned(),
                    limit,
                });
            }
        }
        let chain_data = std::fs::read(chain_path).map_err(into_io_err)?;
        Self::at_chain_data_inner(chain_path, &chain_data, alloc_limit_bytes)
    }

    fn at_chain_data_inner(
        chain_path: &Path,
        chain_data: &[u8],
        alloc_limit_bytes: Option<usize>,
    ) -> Result<Self, Error> {
        if let Some(limit) = alloc_limit_bytes {
            if chain_data.len() > limit {
                return Err(Error::ChainFileTooLarge {
                    path: chain_path.to_owned(),
                    limit,
                });
            }
        }
        let ids = parse_chain_file(chain_data, chain_path)?;

        let chain_dir = chain_path.parent().unwrap_or(Path::new(""));
        let object_hash = ids[0].kind();
        let mut layers = Vec::with_capacity(ids.len());
        let mut num_indices = 0u32;
        let mut num_objects = 0u32;
        for (layer_index, id) in ids.iter().enumerate() {
            let layer_path = chain_dir.join(layer_file_name(id));
            let data = crate::mmap::read_only(&layer_path).map_err(|source| init::Error::Io {
                source,
                path: layer_path.clone(),
            })?;
            let parsed = init::parse_layer(data, layer_path.clone(), alloc_limit_bytes)?;
            if parsed.object_hash != object_hash {
                return Err(Error::LayerObjectHash {
                    actual: parsed.object_hash,
                    expected: object_hash,
                    path: layer_path,
                });
            }
            let mut layer = parsed.layer;
            let actual_checksum = layer.checksum(object_hash.len_in_bytes());
            if actual_checksum != *id {
                return Err(Error::LayerChecksum {
                    actual: actual_checksum,
                    expected: *id,
                    path: layer_path,
                });
            }
            // Git always writes 0 into the base-files field, so only validate it if a future version sets it.
            if layer.num_base_files != 0 && usize::from(layer.num_base_files) != layer_index {
                return Err(Error::BaseFileCount {
                    actual: layer.num_base_files,
                    expected: layer_index,
                    path: layer_path,
                });
            }
            match (layer.base_checksum, layer_index.checked_sub(1).map(|base| ids[base])) {
                (Some(actual), None) => {
                    return Err(Error::UnexpectedBaseChecksum {
                        actual,
                        path: layer_path,
                    });
                }
                (Some(actual), Some(expected)) if actual != expected => {
                    return Err(Error::BaseChecksum {
                        actual,
                        expected,
                        path: layer_path,
                    });
                }
                _ => {}
            }

            layer.indices_in_base = num_indices;
            layer.objects_in_base = num_objects;
            num_indices = num_indices
                .checked_add(layer.num_indices)
                .ok_or_else(|| Error::CountOverflow {
                    what: "pack files",
                    path: chain_path.to_owned(),
                })?;
            num_objects = num_objects
                .checked_add(layer.num_objects)
                .ok_or_else(|| Error::CountOverflow {
                    what: "objects",
                    path: chain_path.to_owned(),
                })?;
            layers.push(layer);
        }

        let version = layers.last().expect("at least one layer").version;
        Ok(File {
            path: chain_path.to_owned(),
            pack_dir: chain_dir.parent().unwrap_or_else(|| Path::new("..")).to_owned(),
            version,
            hash_len: object_hash.len_in_bytes(),
            object_hash,
            num_indices,
            num_objects,
            layers,
        })
    }
}

/// Parse the per-layer checksums from `data`, one hexadecimal hash per line, base layer first.
fn parse_chain_file(data: &[u8], path: &Path) -> Result<Vec<gix_hash::ObjectId>, Error> {
    if data.is_empty() {
        return Err(Error::Empty { path: path.to_owned() });
    }
    let mut ids = Vec::new();
    let mut lines = data.split(|b| *b == b'\n').peekable();
    let mut line_number = 0;
    while let Some(line) = lines.next() {
        line_number += 1;
        if line.is_empty() && lines.peek().is_none() && data.ends_with(b"\n") {
            continue;
        }
        let invalid_line = || Error::InvalidLine {
            line_number,
            path: path.to_owned(),
        };
        if line.is_empty() {
            return Err(invalid_line());
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let id = gix_hash::ObjectId::from_hex(line).map_err(|_| invalid_line())?;
        if ids
            .first()
            .is_some_and(|first: &gix_hash::ObjectId| first.kind() != id.kind())
        {
            return Err(invalid_line());
        }
        ids.push(id);
    }
    if ids.is_empty() {
        return Err(Error::Empty { path: path.to_owned() });
    }
    Ok(ids)
}
