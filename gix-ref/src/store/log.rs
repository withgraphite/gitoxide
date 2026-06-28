use crate::log::Line;

///
pub mod iter {
    use super::Line;

    /// The error returned by reflog iteration.
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error(transparent)]
        Io(#[from] std::io::Error),
        #[cfg(feature = "reftable")]
        #[error(transparent)]
        Reftable(#[from] crate::store::reftable::Error),
    }

    /// A platform to create iterators over reference logs.
    #[must_use = "Iterators should be obtained from this iterator platform"]
    pub struct Platform<'name, 'store> {
        pub(crate) inner: platform::Inner<'name, 'store>,
    }

    pub(crate) mod platform {
        pub enum Inner<'name, 'store> {
            File(crate::file::log::iter::Platform<'name, 'store>),
            #[cfg(feature = "reftable")]
            Reftable {
                store: &'store crate::store::reftable::Store,
                name: &'name crate::FullNameRef,
            },
        }
    }

    /// A forward iterator over owned log lines.
    pub struct Forward {
        inner: std::vec::IntoIter<Result<Line, Error>>,
    }

    /// A reverse iterator over owned log lines.
    pub struct Reverse {
        inner: std::vec::IntoIter<Result<Line, Error>>,
    }

    impl Iterator for Forward {
        type Item = Result<Line, Error>;

        fn next(&mut self) -> Option<Self::Item> {
            self.inner.next()
        }
    }

    impl Iterator for Reverse {
        type Item = Result<Line, Error>;

        fn next(&mut self) -> Option<Self::Item> {
            self.inner.next()
        }
    }

    impl Platform<'_, '_> {
        /// Return a reverse iterator over all log-lines, most recent to oldest.
        pub fn rev(&mut self) -> Result<Option<Reverse>, Error> {
            let lines: Option<Vec<Result<Line, Error>>> = match &mut self.inner {
                platform::Inner::File(platform) => platform.rev()?.map(|iter| {
                    iter.map(|line| line.map_err(|err| Error::Io(std::io::Error::other(err))))
                        .collect()
                }),
                #[cfg(feature = "reftable")]
                platform::Inner::Reftable { store, name } => store
                    .reflog(name, true)?
                    .map(|lines| lines.into_iter().map(Ok).collect::<Vec<Result<Line, Error>>>()),
            };
            Ok(lines.map(|inner| Reverse {
                inner: inner.into_iter(),
            }))
        }

        /// Return a forward iterator over all log-lines, oldest to most recent.
        pub fn all(&mut self) -> Result<Option<Forward>, Error> {
            let lines: Option<Vec<Result<Line, Error>>> = match &mut self.inner {
                platform::Inner::File(platform) => platform.all()?.map(|iter| {
                    iter.map(|line| {
                        line.map(|line| line.to_owned())
                            .map_err(|err| Error::Io(std::io::Error::other(err)))
                    })
                    .collect::<Vec<_>>()
                }),
                #[cfg(feature = "reftable")]
                platform::Inner::Reftable { store, name } => store
                    .reflog(name, false)?
                    .map(|lines| lines.into_iter().map(Ok).collect::<Vec<Result<Line, Error>>>()),
            };
            Ok(lines.map(|inner| Forward {
                inner: inner.into_iter(),
            }))
        }
    }
}
