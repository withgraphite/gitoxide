#![allow(clippy::result_large_err)]

use super::{Error, util};
use crate::config::{
    cache::util::{ApplyLeniency, ApplyLeniencyDefaultValue},
    tree::{Core, Extensions, gitoxide},
};

/// A utility to deal with the cyclic dependency between the ref store and the configuration. The ref-store needs the
/// object hash kind, and the configuration needs the current branch name to resolve conditional includes with `onbranch`.
pub(crate) struct StageOne {
    pub git_dir_config: gix_config::File<'static>,
    pub buf: Vec<u8>,

    pub is_bare: Option<bool>,
    pub lossy: bool,
    pub object_hash: gix_hash::Kind,
    pub ref_storage: crate::config::RefStorage,
    #[cfg(feature = "reftable")]
    pub reftable_options: gix_ref::store::reftable::WriteOptions,
    pub reflog: Option<gix_ref::store::WriteReflog>,
    pub precompose_unicode: bool,
    pub protect_windows: bool,
}

/// Initialization
impl StageOne {
    pub fn new(
        common_dir: &std::path::Path,
        git_dir: &std::path::Path,
        git_dir_trust: gix_sec::Trust,
        lossy: bool,
        lenient: bool,
    ) -> Result<Self, Error> {
        let mut buf = Vec::with_capacity(512);
        let mut config = load_config(
            common_dir.join("config"),
            &mut buf,
            gix_config::Source::Local,
            git_dir_trust,
            lossy,
            lenient,
        )?;

        let is_bare = util::config_bool_opt(&config, &Core::BARE, "core.bare", lenient)?;
        let repo_format_version = config
            .integer("core.repositoryFormatVersion")
            .map(|version| Core::REPOSITORY_FORMAT_VERSION.try_into_usize(version))
            .transpose()?
            .unwrap_or_default();
        let object_hash = match (repo_format_version, config.string(Extensions::OBJECT_FORMAT)) {
            // objectFormat is a repository format version 1 extension.
            (1, Some(format)) => Extensions::OBJECT_FORMAT.try_into_object_format(format)?,
            (0, Some(_)) => return Err(Error::ObjectFormatRequiresV1),
            (0 | 1, None) => legacy_object_hash()?,
            (version, _) => return Err(Error::UnsupportedRepositoryFormatVersion { version }),
        };
        let ref_storage = (repo_format_version == 1)
            .then(|| {
                config
                    .string(Extensions::REF_STORAGE)
                    .map(|format| Extensions::REF_STORAGE.try_into_ref_storage(format))
            })
            .flatten()
            .transpose()?
            .unwrap_or(crate::config::RefStorage::Files);
        #[cfg(feature = "reftable")]
        let reftable_options = if ref_storage == crate::config::RefStorage::Reftable {
            reftable_options(&config)?
        } else {
            gix_ref::store::reftable::WriteOptions::default()
        };

        let extension_worktree = util::config_bool(
            &config,
            &Extensions::WORKTREE_CONFIG,
            "extensions.worktreeConfig",
            false,
            lenient,
        )?;
        if extension_worktree {
            let worktree_config = load_config(
                git_dir.join("config.worktree"),
                &mut buf,
                gix_config::Source::Worktree,
                git_dir_trust,
                lossy,
                lenient,
            )?;
            config.append(worktree_config);
        }
        let precompose_unicode = config
            .boolean(Core::PRECOMPOSE_UNICODE)
            .map(|v| Core::PRECOMPOSE_UNICODE.enrich_error(v))
            .transpose()
            .with_leniency(lenient)
            .map_err(Error::ConfigBoolean)?
            .unwrap_or_default();

        const IS_WINDOWS: bool = cfg!(windows);
        let protect_windows = gitoxide::Core::PROTECT_WINDOWS
            .enrich_error(
                config
                    .boolean(gitoxide::Core::PROTECT_WINDOWS)
                    .unwrap_or(Ok(IS_WINDOWS)),
            )
            .with_lenient_default_value(lenient, IS_WINDOWS)?;

        let reflog = util::query_refupdates(&config, lenient)?;
        Ok(StageOne {
            git_dir_config: config,
            buf,
            is_bare,
            lossy,
            object_hash,
            ref_storage,
            #[cfg(feature = "reftable")]
            reftable_options,
            reflog,
            precompose_unicode,
            protect_windows,
        })
    }
}

/// Return the object hash for a repository that does not set `extensions.objectFormat`.
///
/// Git interprets a missing objectFormat as the original Sha1 layout, so we return
/// gix_hash::Kind::Sha1 whenever this build can handle it.
/// In Sha256-only builds we cannot open such a repository, so return an error instead.
fn legacy_object_hash() -> Result<gix_hash::Kind, Error> {
    #[cfg(feature = "sha1")]
    {
        Ok(gix_hash::Kind::Sha1)
    }
    #[cfg(not(feature = "sha1"))]
    {
        Err(Error::UnsupportedObjectFormat { name: "sha1".into() })
    }
}

#[cfg(feature = "reftable")]
fn reftable_options(config: &gix_config::File<'_>) -> Result<gix_ref::store::reftable::WriteOptions, Error> {
    fn integer(config: &gix_config::File<'_>, key: &'static str) -> Result<Option<i64>, Error> {
        config
            .integer(key)
            .transpose()
            .map_err(|source| Error::ReftableValue { key, source })
    }

    fn boolean(config: &gix_config::File<'_>, key: &'static str) -> Result<Option<bool>, Error> {
        config
            .boolean(key)
            .transpose()
            .map_err(|source| Error::ReftableValue { key, source })
    }

    fn range<T>(key: &'static str, value: i64) -> Result<T, Error>
    where
        T: TryFrom<i64>,
    {
        value.try_into().map_err(|_| Error::ReftableRange { key, value })
    }

    let mut options = gix_ref::store::reftable::WriteOptions::default();
    if let Some(value) = integer(config, "reftable.blockSize")? {
        if !(0..=0x00ff_ffff).contains(&value) || (value != 0 && value < 32) {
            return Err(Error::ReftableRange {
                key: "reftable.blockSize",
                value,
            });
        }
        options.block_size = range("reftable.blockSize", value)?;
    }
    if let Some(value) = integer(config, "reftable.restartInterval")? {
        options.restart_interval = range("reftable.restartInterval", value)?;
    }
    if let Some(value) = boolean(config, "reftable.indexObjects")? {
        options.skip_index_objects = !value;
    }
    if let Some(value) = integer(config, "reftable.geometricFactor")? {
        options.auto_compaction_factor = range("reftable.geometricFactor", value)?;
    }
    if let Some(value) = integer(config, "reftable.lockTimeout")? {
        if value < -1 {
            return Err(Error::ReftableRange {
                key: "reftable.lockTimeout",
                value,
            });
        }
        options.lock_timeout_ms = value;
    }
    Ok(options)
}

fn load_config(
    config_path: std::path::PathBuf,
    buf: &mut Vec<u8>,
    source: gix_config::Source,
    git_dir_trust: gix_sec::Trust,
    lossy: bool,
    lenient: bool,
) -> Result<gix_config::File<'static>, Error> {
    let metadata = gix_config::file::Metadata::from(source)
        .at(&config_path)
        .with(git_dir_trust);
    let mut file = match std::fs::File::open(&config_path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(gix_config::File::new(metadata)),
        Err(err) => {
            let err = Error::Io {
                source: err,
                path: config_path,
            };
            if lenient {
                gix_trace::warn!("ignoring: {err:#?}");
                return Ok(gix_config::File::new(metadata));
            } else {
                return Err(err);
            }
        }
    };

    buf.clear();
    if let Err(err) = std::io::copy(&mut file, buf) {
        let err = Error::Io {
            source: err,
            path: config_path,
        };
        if lenient {
            gix_trace::warn!("ignoring: {err:#?}");
            buf.clear();
        } else {
            return Err(err);
        }
    }

    let config = gix_config::File::from_bytes_owned(
        buf,
        metadata,
        gix_config::file::init::Options {
            includes: gix_config::file::includes::Options::no_follow(),
            ..util::base_options(lossy, lenient)
        },
    )?;

    Ok(config)
}
