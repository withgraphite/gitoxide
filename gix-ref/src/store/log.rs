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
    pub struct Forward<'a> {
        inner: forward::Inner<'a>,
    }

    mod forward {
        pub enum Inner<'a> {
            File(crate::file::log::iter::Forward<'a>),
            #[cfg(feature = "reftable")]
            Reftable(std::vec::IntoIter<crate::log::Line>),
        }
    }

    /// A reverse iterator over owned log lines.
    pub struct Reverse<'a> {
        inner: reverse::Inner<'a>,
    }

    mod reverse {
        pub enum Inner<'a> {
            File(crate::file::log::iter::Reverse<'a, std::fs::File>),
            #[cfg(feature = "reftable")]
            Reftable(std::vec::IntoIter<crate::log::Line>),
        }
    }

    impl Iterator for Forward<'_> {
        type Item = Result<Line, Error>;

        fn next(&mut self) -> Option<Self::Item> {
            match &mut self.inner {
                forward::Inner::File(iter) => iter.next().map(|line| {
                    line.map(|line| line.to_owned())
                        .map_err(|err| Error::Io(std::io::Error::other(err)))
                }),
                #[cfg(feature = "reftable")]
                forward::Inner::Reftable(iter) => iter.next().map(Ok),
            }
        }
    }

    impl Iterator for Reverse<'_> {
        type Item = Result<Line, Error>;

        fn next(&mut self) -> Option<Self::Item> {
            match &mut self.inner {
                reverse::Inner::File(iter) => iter
                    .next()
                    .map(|line| line.map_err(|err| Error::Io(std::io::Error::other(err)))),
                #[cfg(feature = "reftable")]
                reverse::Inner::Reftable(iter) => iter.next().map(Ok),
            }
        }
    }

    impl Platform<'_, '_> {
        /// Return a reverse iterator over all log-lines, most recent to oldest.
        pub fn rev(&mut self) -> Result<Option<Reverse<'_>>, Error> {
            match &mut self.inner {
                platform::Inner::File(platform) => Ok(platform.rev()?.map(|inner| Reverse {
                    inner: reverse::Inner::File(inner),
                })),
                #[cfg(feature = "reftable")]
                platform::Inner::Reftable { store, name } => Ok(store.reflog(name, true)?.map(|lines| Reverse {
                    inner: reverse::Inner::Reftable(lines.into_iter()),
                })),
            }
        }

        /// Return a forward iterator over all log-lines, oldest to most recent.
        pub fn all(&mut self) -> Result<Option<Forward<'_>>, Error> {
            match &mut self.inner {
                platform::Inner::File(platform) => Ok(platform.all()?.map(|inner| Forward {
                    inner: forward::Inner::File(inner),
                })),
                #[cfg(feature = "reftable")]
                platform::Inner::Reftable { store, name } => Ok(store.reflog(name, false)?.map(|lines| Forward {
                    inner: forward::Inner::Reftable(lines.into_iter()),
                })),
            }
        }
    }
}
