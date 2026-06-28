#![allow(dead_code)]
use crate::{Namespace, store};

#[derive(Clone)]
pub(crate) enum Backend {
    File(crate::file::Store),
    #[cfg(feature = "reftable")]
    Reftable(crate::store::reftable::Store),
}

impl crate::Store {
    /// Return a new handle which sees all references if `namespace` is `None` or all read and write operations are limited
    /// to the given `namespace` if `Some`.
    pub(crate) fn to_handle(&self) -> store::Handle {
        Self::new_handle_inner(&self.backend, None)
    }

    /// As above, but supports a namespace to be set
    pub(crate) fn to_handle_namespaced(&self, namespace: Option<Namespace>) -> store::Handle {
        Self::new_handle_inner(&self.backend, namespace)
    }

    fn new_handle_inner(backend: &store::Backend, namespace: Option<Namespace>) -> store::Handle {
        store::Handle {
            backend: match backend {
                store::Backend::File(store) => {
                    let mut store = store.clone();
                    store.namespace = namespace;
                    store::handle::Backend::File(store)
                }
                #[cfg(feature = "reftable")]
                store::Backend::Reftable(store) => {
                    let mut store = store.clone();
                    store.namespace = namespace;
                    store::handle::Backend::Reftable(store)
                }
            },
        }
    }
}

///
pub mod find;

mod iter {
    // impl store::Handle {
    //     pub fn iter<'p, 's>(&'s self, packed: Option<&'p packed::Buffer>) -> std::io::Result<LooseThenPacked<'p, 's>> {
    // }
}
