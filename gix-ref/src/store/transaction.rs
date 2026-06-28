//! Backend-neutral transactions for editing references on [`crate::Store`].

use crate::{file, transaction::RefEdit};

/// How to handle packed refs during a transaction.
///
/// Only meaningful for the files backend; ignored for reftable (or rejected when writes are unsupported).
pub type PackedRefs<'a> = file::transaction::PackedRefs<'a>;

/// The error returned by backend-neutral transaction operations.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Prepare(#[from] file::transaction::prepare::Error),
    #[error(transparent)]
    Commit(#[from] file::transaction::commit::Error),
    #[cfg(feature = "reftable")]
    #[error(transparent)]
    Reftable(#[from] crate::store::reftable::TransactionError),
}

/// A platform to configure and run a reference transaction against any backend.
#[must_use = "Transactions must be prepared and committed"]
pub struct Platform<'s, 'p> {
    inner: platform::Inner<'s, 'p>,
    objects: Option<&'p dyn gix_object::Find>,
}

pub(crate) mod platform {
    pub enum Inner<'s, 'p> {
        File(crate::file::Transaction<'s, 'p>),
        #[cfg(feature = "reftable")]
        Reftable(crate::store::reftable::Transaction<'s, 'p>),
    }
}

impl crate::Store {
    /// Open a transaction to edit references on this store.
    ///
    /// Dispatch to the active reference backend.
    pub fn transaction(&self) -> Platform<'_, '_> {
        Platform {
            inner: match &self.backend {
                crate::store::Backend::File(store) => platform::Inner::File(store.transaction()),
                #[cfg(feature = "reftable")]
                crate::store::Backend::Reftable(store) => platform::Inner::Reftable(store.transaction()),
            },
            objects: None,
        }
    }
}

impl<'p> Platform<'_, 'p> {
    /// Provide object lookup for backends that can persist peeled reference values.
    pub fn objects(mut self, objects: &'p dyn gix_object::Find) -> Self {
        self.objects = Some(objects);
        self
    }

    /// Configure how packed refs are handled during the transaction.
    ///
    /// Only affects the files backend; has no effect when the backend is reftable.
    pub fn packed_refs(mut self, packed_refs: PackedRefs<'p>) -> Self {
        self.inner = match self.inner {
            platform::Inner::File(txn) => platform::Inner::File(txn.packed_refs(packed_refs)),
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(transaction) => platform::Inner::Reftable(transaction),
        };
        self
    }

    /// Prepare `edits` for commit, acquiring locks as needed.
    ///
    /// Reftable stacks use the lock timeout from [`crate::store::init::Options::reftable`];
    /// the two lock modes apply to the files backend.
    ///
    /// Returns a prepared platform that can be [committed](Platform::commit).
    pub fn prepare(
        self,
        edits: impl IntoIterator<Item = RefEdit>,
        fail: gix_lock::acquire::Fail,
        packed_refs_fail: gix_lock::acquire::Fail,
    ) -> Result<Self, Error> {
        match self.inner {
            platform::Inner::File(txn) => Ok(Platform {
                inner: platform::Inner::File(txn.prepare(edits, fail, packed_refs_fail)?),
                objects: self.objects,
            }),
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(transaction) => {
                let transaction = match self.objects {
                    Some(objects) => transaction.objects(objects),
                    None => transaction,
                };
                Ok(Platform {
                    inner: platform::Inner::Reftable(transaction.prepare(edits, fail)?),
                    objects: self.objects,
                })
            }
        }
    }

    /// Commit prepared edits using `committer` for reflog entries when needed.
    ///
    /// The platform must have been [prepared](Platform::prepare) first.
    pub fn commit(self, committer: Option<gix_actor::SignatureRef<'_>>) -> Result<Vec<RefEdit>, Error> {
        match self.inner {
            platform::Inner::File(txn) => Ok(txn.commit(committer)?),
            #[cfg(feature = "reftable")]
            platform::Inner::Reftable(transaction) => Ok(transaction.commit(committer)?),
        }
    }
}
