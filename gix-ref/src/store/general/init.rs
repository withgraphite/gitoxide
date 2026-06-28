use std::path::PathBuf;

use crate::store::init::{Error, Storage};

impl crate::Store {
    /// Create a new store at the given location, typically the `.git/` directory.
    /// Use [`opts`](crate::store::init::Options) to adjust settings.
    ///
    /// Storage backend is selected via [`opts.storage`](crate::store::init::Options::storage), defaulting to
    /// automatic detection (reftable directory present when the feature is enabled, otherwise files).
    ///
    /// Note that if [`precompose_unicode`](crate::store::init::Options::precompose_unicode) is set in the options,
    /// the `git_dir` is also expected to use precomposed unicode, or else some operations that strip prefixes will fail.
    pub fn at(git_dir: PathBuf, opts: crate::store::init::Options) -> Result<Self, Error> {
        Self::open(git_dir, None, opts)
    }

    /// Create a store for a linked worktree with the given `common_dir`.
    pub fn for_linked_worktree(
        git_dir: PathBuf,
        common_dir: PathBuf,
        opts: crate::store::init::Options,
    ) -> Result<Self, Error> {
        Self::open(git_dir, Some(common_dir), opts)
    }

    fn open(git_dir: PathBuf, common_dir: Option<PathBuf>, opts: crate::store::init::Options) -> Result<Self, Error> {
        std::fs::read_dir(&git_dir)?;
        let storage = match opts.storage {
            Storage::Auto => detect_storage(common_dir.as_deref().unwrap_or(&git_dir)),
            Storage::Files => Storage::Files,
            #[cfg(feature = "reftable")]
            Storage::Reftable => Storage::Reftable,
        };
        match storage {
            Storage::Files | Storage::Auto => Ok(Self::from(match &common_dir {
                Some(common_dir) => crate::file::Store::for_linked_worktree(git_dir, common_dir.clone(), opts),
                None => crate::file::Store::at(git_dir, opts),
            })),
            #[cfg(feature = "reftable")]
            Storage::Reftable => Ok(Self::from(match common_dir {
                Some(common_dir) => crate::store::reftable::Store::for_linked_worktree(git_dir, common_dir, opts)?,
                None => crate::store::reftable::Store::at(git_dir, opts)?,
            })),
        }
    }

    /// Create a new reftable-backed store at the given location.
    #[cfg(feature = "reftable")]
    pub fn at_reftable(
        git_dir: PathBuf,
        opts: crate::store::init::Options,
    ) -> Result<Self, crate::store::reftable::Error> {
        Ok(Self::from(crate::store::reftable::Store::at(git_dir, opts)?))
    }

    /// Create a new reftable-backed store for a linked worktree.
    #[cfg(feature = "reftable")]
    pub fn reftable_for_linked_worktree(
        git_dir: PathBuf,
        common_dir: PathBuf,
        opts: crate::store::init::Options,
    ) -> Result<Self, crate::store::reftable::Error> {
        Ok(Self::from(crate::store::reftable::Store::for_linked_worktree(
            git_dir, common_dir, opts,
        )?))
    }
}

impl From<crate::file::Store> for crate::Store {
    fn from(store: crate::file::Store) -> Self {
        crate::Store {
            backend: crate::store::Backend::File(store),
        }
    }
}

#[cfg(feature = "reftable")]
impl From<crate::store::reftable::Store> for crate::Store {
    fn from(store: crate::store::reftable::Store) -> Self {
        crate::Store {
            backend: crate::store::Backend::Reftable(store),
        }
    }
}

fn detect_storage(common_or_git_dir: &std::path::Path) -> Storage {
    #[cfg(feature = "reftable")]
    {
        if common_or_git_dir.join("reftable").is_dir() {
            return Storage::Reftable;
        }
    }
    let _ = common_or_git_dir;
    Storage::Files
}
