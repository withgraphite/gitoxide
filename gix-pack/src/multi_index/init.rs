use std::path::{Path, PathBuf};

use crate::multi_index::{File, Layer, Version, chunk};

mod error {
    use crate::multi_index::chunk;

    /// The error returned by [File::at()][super::File::at()].
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error("Could not open multi-index file at '{path}'")]
        Io {
            source: std::io::Error,
            path: std::path::PathBuf,
        },
        #[error("{message}")]
        Corrupt { message: &'static str },
        #[error("Unsupported multi-index version: {version})")]
        UnsupportedVersion { version: u8 },
        #[error("Unsupported hash kind: {kind})")]
        UnsupportedObjectHash { kind: u8 },
        #[error(transparent)]
        ChunkFileQuery(#[from] gix_error::Message),
        #[error(transparent)]
        ChunkFileDecode(#[from] gix_error::ValidationError),
        #[error("The multi-pack fan doesn't have the correct size of 256 * 4 bytes")]
        MultiPackFanSize,
        #[error(transparent)]
        PackNames(#[from] chunk::index_names::decode::Error),
        #[error("multi-index chunk {:?} has invalid size: {message}", String::from_utf8_lossy(.id))]
        InvalidChunkSize { id: gix_chunk::Id, message: &'static str },
    }
}

pub use error::Error;

/// The result of parsing a single multi-pack index file, possibly one layer of a chain.
pub(crate) struct ParsedLayer<T> {
    pub(crate) layer: Layer<T>,
    pub(crate) version: Version,
    pub(crate) object_hash: gix_hash::Kind,
}

/// Initialization
impl File<crate::MMap> {
    /// Open either a standalone multi-pack index or an incremental chain based on the file name at `path`.
    pub fn at_path(
        path: impl AsRef<Path>,
        alloc_limit_bytes: Option<usize>,
    ) -> Result<Self, crate::multi_index::open::Error> {
        let path = path.as_ref();
        if path.file_name() == Some(std::ffi::OsStr::new(crate::multi_index::chain::CHAIN_FILE)) {
            Self::at_chain(path, alloc_limit_bytes).map_err(Into::into)
        } else {
            Self::at(path, alloc_limit_bytes).map_err(Into::into)
        }
    }

    /// Open the multi-index file at the given `path`.
    ///
    /// `alloc_limit_bytes` bounds each allocation caused by user-controlled on-disk data, useful for untrusted input.
    /// Use `None` to disable the limit.
    pub fn at(path: impl AsRef<Path>, alloc_limit_bytes: Option<usize>) -> Result<Self, Error> {
        Self::at_inner(path.as_ref(), alloc_limit_bytes)
    }

    fn at_inner(path: &Path, alloc_limit_bytes: Option<usize>) -> Result<Self, Error> {
        let data = crate::mmap::read_only(path).map_err(|source| Error::Io {
            source,
            path: path.to_owned(),
        })?;
        Self::from_data(data, path.to_owned(), alloc_limit_bytes)
    }
}

impl<T> File<T>
where
    T: crate::FileData,
{
    /// Instantiate a multi-index file from `data` as assumed to be read or memory-mapped from `path`.
    ///
    /// `alloc_limit_bytes` bounds each allocation caused by untrusted on-disk multi-index data.
    /// Use `None` to disable the limit.
    ///
    ///  It is used to reject reserving the output `Vec<PathBuf>` if its capacity estimate exceeds the limit,
    ///  and to reject any single path entry whose byte length exceeds the limit before turning it into a `PathBuf`.
    pub fn from_data(data: T, path: PathBuf, alloc_limit_bytes: Option<usize>) -> Result<Self, Error> {
        let pack_dir = path.parent().unwrap_or_else(|| Path::new("")).to_owned();
        let ParsedLayer {
            layer,
            version,
            object_hash,
        } = parse_layer(data, path.clone(), alloc_limit_bytes)?;
        Ok(File {
            path,
            pack_dir,
            version,
            hash_len: object_hash.len_in_bytes(),
            object_hash,
            num_indices: layer.num_indices,
            num_objects: layer.num_objects,
            layers: vec![layer],
        })
    }
}

/// Parse a single multi-pack index file, which may be standalone or one layer of a chain.
///
/// The returned layer has its `objects_in_base` and `indices_in_base` set to zero, which suits
/// a standalone file or the base of a chain - [chain loading](File::at_chain()) adjusts them.
pub(crate) fn parse_layer<T>(data: T, path: PathBuf, alloc_limit_bytes: Option<usize>) -> Result<ParsedLayer<T>, Error>
where
    T: crate::FileData,
{
    const TRAILER_LEN: usize = gix_hash::Kind::shortest().len_in_bytes(); /* trailing hash */
    if data.len()
        < File::<T>::HEADER_LEN
            + gix_chunk::file::Index::size_for_entries(4 /*index names, fan, offsets, oids*/)
            + chunk::fanout::SIZE
            + TRAILER_LEN
    {
        return Err(Error::Corrupt {
            message: "multi-index file is truncated and too short",
        });
    }

    let (version, object_hash, num_chunks, num_base_files, num_indices) = {
        let (signature, data) = data.split_at(4);
        if signature != File::<T>::SIGNATURE {
            return Err(Error::Corrupt {
                message: "Invalid signature",
            });
        }
        let (version, data) = data.split_at(1);
        let version = match version[0] {
            1 => Version::V1,
            2 => Version::V2,
            version => return Err(Error::UnsupportedVersion { version }),
        };

        let (object_hash, data) = data.split_at(1);
        let object_hash = gix_hash::Kind::try_from(object_hash[0])
            .map_err(|unknown| Error::UnsupportedObjectHash { kind: unknown })?;
        let (num_chunks, data) = data.split_at(1);
        let num_chunks = num_chunks[0];

        // Note that git always writes 0 here, even for layers of a chain whose position implies their amount of base files.
        let (num_base_files, data) = data.split_at(1);
        let num_base_files = num_base_files[0];

        let (num_indices, _) = data.split_at(4);
        let num_indices = crate::read_u32(num_indices);

        (version, object_hash, num_chunks, num_base_files, num_indices)
    };

    let chunks = gix_chunk::file::Index::from_bytes(&data, File::<T>::HEADER_LEN, u32::from(num_chunks))?;

    let index_names = chunks.data_by_id(&data, chunk::index_names::ID)?;
    let index_names =
        chunk::index_names::from_bytes(index_names, num_indices, version == Version::V1, alloc_limit_bytes)?;

    let fan = chunks.data_by_id(&data, chunk::fanout::ID)?;
    let fan = chunk::fanout::from_bytes(fan).ok_or(Error::MultiPackFanSize)?;
    let num_objects = fan[255];
    validate_fan(&fan)?;

    let lookup = chunks.validated_usize_offset_by_id(chunk::lookup::ID, |offset| {
        chunk::lookup::is_valid(&offset, object_hash, num_objects)
            .then_some(offset)
            .ok_or(Error::InvalidChunkSize {
                id: chunk::lookup::ID,
                message: "The chunk with alphabetically ordered object ids doesn't have the correct size",
            })
    })??;
    let offsets = chunks.validated_usize_offset_by_id(chunk::offsets::ID, |offset| {
        chunk::offsets::is_valid(&offset, num_objects)
            .then_some(offset)
            .ok_or(Error::InvalidChunkSize {
                id: chunk::offsets::ID,
                message: "The chunk with offsets into the pack doesn't have the correct size",
            })
    })??;
    let large_offsets = chunks
        .validated_usize_offset_by_id(chunk::large_offsets::ID, |offset| {
            chunk::large_offsets::is_valid(&offset)
                .then_some(offset)
                .ok_or(Error::InvalidChunkSize {
                    id: chunk::large_offsets::ID,
                    message: "The chunk with large offsets into the pack doesn't have the correct size",
                })
        })
        .ok()
        .transpose()?;
    let base_checksum = chunks
        .validated_usize_offset_by_id(chunk::base::ID, |offset| {
            chunk::base::is_valid(&offset, object_hash)
                .then_some(offset)
                .ok_or(Error::InvalidChunkSize {
                    id: chunk::base::ID,
                    message: "The chunk with the base layer checksum must be exactly one hash long",
                })
        })
        .ok()
        .transpose()?
        .map(|range| gix_hash::ObjectId::from_bytes_or_panic(&data[range]));

    let checksum_offset = chunks.highest_offset() as usize;
    let trailer = &data[checksum_offset..];
    if trailer.len() != object_hash.len_in_bytes() {
        return Err(Error::Corrupt {
            message: "Trailing checksum didn't have the expected size or there were unknown bytes after the checksum.",
        });
    }

    Ok(ParsedLayer {
        layer: Layer {
            data,
            path,
            version,
            num_base_files,
            num_indices,
            index_names,
            num_objects,
            indices_in_base: 0,
            objects_in_base: 0,
            fan,
            base_checksum,
            lookup_ofs: lookup.start,
            offsets_ofs: offsets.start,
            large_offsets,
        },
        version,
        object_hash,
    })
}

fn validate_fan(fan: &[u32; 256]) -> Result<(), Error> {
    if !crate::fan_is_monotonically_increasing(fan) {
        return Err(Error::Corrupt {
            message: "multi-index fan-out table must be monotonically increasing",
        });
    }
    Ok(())
}
