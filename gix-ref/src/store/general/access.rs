use std::path::Path;

impl crate::Store {
    /// Return the kind of reference storage backend in use.
    pub fn kind(&self) -> crate::store::Kind {
        match &self.backend {
            crate::store::Backend::File(_) => crate::store::Kind::Files,
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(_) => crate::store::Kind::Reftable,
        }
    }

    /// Return the `.git` directory at which references are loaded.
    pub fn git_dir(&self) -> &Path {
        match &self.backend {
            crate::store::Backend::File(store) => store.git_dir(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.git_dir(),
        }
    }

    /// Return the common directory if this store is for a linked worktree.
    pub fn common_dir(&self) -> Option<&Path> {
        match &self.backend {
            crate::store::Backend::File(store) => store.common_dir(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.common_dir(),
        }
    }

    /// Return the common directory, falling back to [`git_dir()`][Self::git_dir].
    pub fn common_dir_resolved(&self) -> &Path {
        match &self.backend {
            crate::store::Backend::File(store) => store.common_dir_resolved(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.common_dir_resolved(),
        }
    }

    /// Return the currently configured namespace.
    pub fn namespace(&self) -> Option<&crate::Namespace> {
        match &self.backend {
            crate::store::Backend::File(store) => store.namespace.as_ref(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.namespace.as_ref(),
        }
    }

    /// Remove the currently configured namespace.
    pub fn take_namespace(&mut self) -> Option<crate::Namespace> {
        match &mut self.backend {
            crate::store::Backend::File(store) => store.namespace.take(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.namespace.take(),
        }
    }

    /// Set the currently configured namespace.
    pub fn set_namespace(&mut self, namespace: crate::Namespace) -> Option<crate::Namespace> {
        match &mut self.backend {
            crate::store::Backend::File(store) => store.namespace.replace(namespace),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.namespace.replace(namespace),
        }
    }

    /// Set the reflog write mode and return the old one.
    pub fn set_write_reflog(&mut self, mut mode: crate::store::WriteReflog) -> crate::store::WriteReflog {
        match &mut self.backend {
            crate::store::Backend::File(store) => {
                std::mem::swap(&mut store.write_reflog, &mut mode);
                mode
            }
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => {
                std::mem::swap(&mut store.write_reflog, &mut mode);
                mode
            }
        }
    }

    /// Return `Some(true)` if this is a freshly initialized ref store.
    pub fn is_pristine(&self, default_ref: &crate::FullNameRef) -> Option<bool> {
        match &self.backend {
            crate::store::Backend::File(store) => store.is_pristine(default_ref),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => store.is_pristine(default_ref),
        }
    }

    /// Set the amount of `bytes` needed for the `.git/packed-refs` file to be memory mapped.
    ///
    /// Only applies to the files backend; returns `bytes` unchanged for other backends.
    pub fn set_packed_buffer_mmap_threshold(&mut self, bytes: u64) -> u64 {
        match &mut self.backend {
            crate::store::Backend::File(store) => store.set_packed_buffer_mmap_threshold(bytes),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(_) => bytes,
        }
    }

    /// Return a platform to obtain iterators over references.
    pub fn iter(&self) -> Result<crate::store::iter::Platform<'_>, crate::store::iter::Error> {
        Ok(crate::store::iter::Platform {
            inner: match &self.backend {
                crate::store::Backend::File(store) => crate::store::iter::platform::Inner::File(store.iter()?),
                #[cfg(feature = "reftable")]
                crate::store::Backend::Reftable(store) => crate::store::iter::platform::Inner::Reftable(store),
            },
        })
    }

    /// Return a cached packed refs buffer when using the files backend.
    ///
    /// Returns `Ok(None)` for backends that do not use packed-refs.
    pub fn cached_packed_buffer(
        &self,
    ) -> Result<Option<crate::file::packed::SharedBufferSnapshot>, crate::packed::buffer::open::Error> {
        match &self.backend {
            crate::store::Backend::File(store) => store.cached_packed_buffer(),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(_) => Ok(None),
        }
    }

    /// Return `true` if a reflog exists for `name`.
    pub fn reflog_exists(&self, name: &crate::FullNameRef) -> Result<bool, crate::store::log::iter::Error> {
        match &self.backend {
            crate::store::Backend::File(store) => Ok(store.reflog_exists(name).expect("FullNameRef is always valid")),
            #[cfg(feature = "reftable")]
            crate::store::Backend::Reftable(store) => Ok(store.reflog_exists(name)?),
        }
    }

    /// Return a platform for obtaining iterators over reference logs.
    pub fn reflog_iter<'name>(&self, name: &'name crate::FullNameRef) -> crate::store::log::iter::Platform<'name, '_> {
        crate::store::log::iter::Platform {
            inner: match &self.backend {
                crate::store::Backend::File(store) => {
                    crate::store::log::iter::platform::Inner::File(crate::file::log::iter::Platform {
                        store,
                        name,
                        buf: Vec::new(),
                    })
                }
                #[cfg(feature = "reftable")]
                crate::store::Backend::Reftable(store) => {
                    crate::store::log::iter::platform::Inner::Reftable { store, name }
                }
            },
        }
    }
}
