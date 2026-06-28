use crate::{PartialNameRef, Reference, store};

mod error {
    use std::convert::Infallible;

    /// The error returned by [`crate::file::Store::find_loose()`].
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error("An error occurred while finding a reference in the loose file database")]
        File(#[from] crate::file::find::Error),
        #[cfg(feature = "reftable")]
        #[error("An error occurred while finding a reference in the reftable database")]
        Reftable(#[from] crate::store::reftable::Error),
        #[error("The ref name or path is not a valid ref name")]
        RefnameValidation(#[from] crate::name::Error),
    }

    impl From<Infallible> for Error {
        fn from(_: Infallible) -> Self {
            unreachable!("this impl is needed to allow passing a known valid partial path as parameter")
        }
    }
}

pub use error::Error;

use crate::store::handle;

impl store::Handle {
    /// TODO: actually implement this with handling of the packed buffer.
    pub fn try_find<'a, Name, E>(&self, partial: Name) -> Result<Option<Reference>, Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        Error: From<E>,
    {
        let partial = partial.try_into()?;
        match &self.backend {
            handle::Backend::File(store) => store.try_find(partial).map_err(Into::into),
            #[cfg(feature = "reftable")]
            handle::Backend::Reftable(store) => store.try_find(partial).map_err(Into::into),
        }
    }
}

impl crate::Store {
    /// Try to find a reference by partial name.
    pub fn try_find<'a, Name, E>(&self, partial: Name) -> Result<Option<Reference>, Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        Error: From<E>,
    {
        let partial = partial.try_into()?;
        match &self.backend {
            store::Backend::File(store) => store.try_find(partial).map_err(Into::into),
            #[cfg(feature = "reftable")]
            store::Backend::Reftable(store) => store.try_find(partial).map_err(Into::into),
        }
    }
}

/// Errors and helpers for finding references that must exist.
pub mod existing {
    mod error {
        use crate::PartialName;

        /// The error returned when [`crate::Store::find()`] cannot locate a reference.
        #[derive(Debug, thiserror::Error)]
        #[allow(missing_docs)]
        pub enum Error {
            #[error("An error occurred while finding a reference in the database")]
            Find(#[from] crate::store::find::Error),
            #[error("The ref partially named {:?} could not be found", name.as_ref().as_bstr())]
            NotFound { name: PartialName },
        }
    }

    pub use error::Error;

    use crate::{PartialNameRef, Reference, store};

    impl store::Handle {
        /// Similar to [`crate::file::Store::find()`] but a non-existing ref is treated as error.
        pub fn find<'a, Name, E>(&self, partial: Name) -> Result<Reference, Error>
        where
            Name: TryInto<&'a PartialNameRef, Error = E> + Clone,
            crate::store::find::Error: From<E>,
        {
            let path = partial.clone().try_into().map_err(crate::store::find::Error::from)?;
            match self.try_find(partial)? {
                Some(r) => Ok(r),
                None => Err(Error::NotFound { name: path.to_owned() }),
            }
        }
    }

    impl crate::Store {
        /// Similar to [`try_find()`][crate::Store::try_find], but a non-existing ref is treated as error.
        pub fn find<'a, Name, E>(&self, partial: Name) -> Result<Reference, Error>
        where
            Name: TryInto<&'a PartialNameRef, Error = E> + Clone,
            crate::store::find::Error: From<E>,
        {
            let path = partial.clone().try_into().map_err(crate::store::find::Error::from)?;
            match self.try_find(partial)? {
                Some(r) => Ok(r),
                None => Err(Error::NotFound { name: path.to_owned() }),
            }
        }
    }
}

impl From<crate::file::find::existing::Error> for existing::Error {
    fn from(value: crate::file::find::existing::Error) -> Self {
        match value {
            crate::file::find::existing::Error::Find(err) => existing::Error::Find(err.into()),
            crate::file::find::existing::Error::NotFound { name } => existing::Error::NotFound {
                name: name
                    .to_string_lossy()
                    .into_owned()
                    .try_into()
                    .expect("file store reports valid partial names"),
            },
        }
    }
}
