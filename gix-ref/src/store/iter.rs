use gix_path::RelativePath;

use crate::{Reference, file};

/// The error returned by reference iteration.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    FileOpen(#[from] crate::packed::buffer::open::Error),
    #[error(transparent)]
    FileIter(#[from] file::iter::loose_then_packed::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(feature = "reftable")]
    #[error(transparent)]
    Reftable(#[from] crate::store::reftable::Error),
}

/// A platform to create iterators over references.
#[must_use = "Iterators should be obtained from this iterator platform"]
pub struct Platform<'repo> {
    pub(crate) inner: platform::Inner<'repo>,
}

pub(crate) mod platform {
    pub enum Inner<'repo> {
        File(crate::file::iter::Platform<'repo>),
        #[cfg(feature = "reftable")]
        Reftable(&'repo crate::store::reftable::Store),
    }
}

/// An iterator over references.
pub struct Iter<'repo> {
    inner: Box<dyn Iterator<Item = Result<Reference, Error>> + 'repo>,
}

impl<'repo> Iter<'repo> {
    pub(crate) fn new(inner: impl Iterator<Item = Result<Reference, Error>> + 'repo) -> Self {
        Self { inner: Box::new(inner) }
    }
}

impl Iterator for Iter<'_> {
    type Item = Result<Reference, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl Platform<'_> {
    /// Return an iterator over all references, sorted by their name.
    pub fn all(&self) -> Result<Iter<'_>, Error> {
        match &self.inner {
            platform::Inner::File(platform) => Ok(Iter::new(platform.all()?.map(|res| res.map_err(Into::into)))),
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(store) => Ok(Iter::new(store.iter(None)?.into_iter().map(Ok))),
        }
    }

    /// Return an iterator over all references matching `prefix`.
    pub fn prefixed(&self, prefix: &RelativePath) -> Result<Iter<'_>, Error> {
        match &self.inner {
            platform::Inner::File(platform) => {
                Ok(Iter::new(platform.prefixed(prefix)?.map(|res| res.map_err(Into::into))))
            }
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(store) => Ok(Iter::new(store.iter(Some(prefix.as_ref()))?.into_iter().map(Ok))),
        }
    }

    /// Return an iterator over pseudo references like `HEAD`.
    pub fn pseudo(&self) -> Result<Iter<'_>, Error> {
        match &self.inner {
            platform::Inner::File(platform) => Ok(Iter::new(platform.pseudo()?.map(|res| res.map_err(Into::into)))),
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(store) => Ok(Iter::new(store.pseudo()?.into_iter().map(Ok))),
        }
    }
}
