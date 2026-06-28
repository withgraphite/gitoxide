use std::collections::BTreeSet;

use gix_hash::ObjectId;

use crate::{FullNameRef, Reference, Target, file, peel};

/// A store that can provide read operations needed by [`Reference`] helpers.
pub trait ReferenceStore {
    /// Try to find a reference by full name.
    fn try_find_reference(&self, name: &FullNameRef) -> Result<Option<Reference>, crate::store::find::Error>;
    /// Return true if a reflog exists for `name`.
    fn reflog_exists(&self, name: &FullNameRef) -> Result<bool, crate::store::log::iter::Error>;
    /// Return a platform for iterating the reflog for `name`.
    fn reflog_iter<'name>(&self, name: &'name FullNameRef) -> crate::store::log::iter::Platform<'name, '_>;
    /// Return a packed-refs buffer when the backend supports it (files only).
    fn packed_refs(&self) -> Option<&crate::packed::Buffer> {
        None
    }
}

impl ReferenceStore for crate::Store {
    fn try_find_reference(&self, name: &FullNameRef) -> Result<Option<Reference>, crate::store::find::Error> {
        self.try_find(name)
    }

    fn reflog_exists(&self, name: &FullNameRef) -> Result<bool, crate::store::log::iter::Error> {
        self.reflog_exists(name)
    }

    fn reflog_iter<'name>(&self, name: &'name FullNameRef) -> crate::store::log::iter::Platform<'name, '_> {
        self.reflog_iter(name)
    }

    fn packed_refs(&self) -> Option<&crate::packed::Buffer> {
        None
    }
}

impl ReferenceStore for file::Store {
    fn try_find_reference(&self, name: &FullNameRef) -> Result<Option<Reference>, crate::store::find::Error> {
        self.try_find(name).map_err(Into::into)
    }

    fn reflog_exists(&self, name: &FullNameRef) -> Result<bool, crate::store::log::iter::Error> {
        Ok(self.reflog_exists(name).expect("FullNameRef is always valid"))
    }

    fn reflog_iter<'name>(&self, name: &'name FullNameRef) -> crate::store::log::iter::Platform<'name, '_> {
        crate::store::log::iter::Platform {
            inner: crate::store::log::iter::platform::Inner::File(crate::file::log::iter::Platform {
                store: self,
                name,
                buf: Vec::new(),
            }),
        }
    }
}

impl Reference {
    /// Return a platform for obtaining iterators over this reference's logs.
    pub fn log_iter<'name, 'store, S>(&'name self, store: &'store S) -> crate::store::log::iter::Platform<'name, 'store>
    where
        S: ReferenceStore + ?Sized,
    {
        store.reflog_iter(self.name.as_ref())
    }

    /// Return true if this reference has a reflog.
    pub fn log_exists<S>(&self, store: &S) -> bool
    where
        S: ReferenceStore + ?Sized,
    {
        store.reflog_exists(self.name.as_ref()).unwrap_or(false)
    }

    /// Follow all symbolic targets and peel annotated tags to the first non-tag object.
    pub fn peel_to_id<S>(&mut self, store: &S, objects: &dyn gix_object::Find) -> Result<ObjectId, peel::to_id::Error>
    where
        S: ReferenceStore + ?Sized,
    {
        self.peel_to_id_packed(store, objects, store.packed_refs())
    }

    /// Like [`peel_to_id()`][Self::peel_to_id], but can reuse a packed-refs snapshot.
    pub fn peel_to_id_packed<S>(
        &mut self,
        store: &S,
        objects: &dyn gix_object::Find,
        packed: Option<&crate::packed::Buffer>,
    ) -> Result<ObjectId, peel::to_id::Error>
    where
        S: ReferenceStore + ?Sized,
    {
        match self.peeled {
            Some(peeled) => {
                self.target = Target::Object(peeled);
                Ok(peeled)
            }
            None => {
                let mut oid = self.follow_to_object_packed(store, packed)?;
                let mut buf = Vec::new();
                let peeled_id = loop {
                    let gix_object::Data {
                        kind,
                        data,
                        object_hash: hash_kind,
                    } = objects
                        .try_find(&oid, &mut buf)?
                        .ok_or_else(|| peel::to_id::Error::NotFound {
                            oid,
                            name: self.name.0.clone(),
                        })?;
                    match kind {
                        gix_object::Kind::Tag => {
                            oid = gix_object::TagRefIter::from_bytes(data, hash_kind)
                                .target_id()
                                .map_err(|_err| peel::to_id::Error::NotFound {
                                    oid,
                                    name: self.name.0.clone(),
                                })?;
                        }
                        _ => break oid,
                    }
                };
                self.peeled = Some(peeled_id);
                self.target = Target::Object(peeled_id);
                Ok(peeled_id)
            }
        }
    }

    /// Follow all symbolic references until this reference points to an object.
    pub fn follow_to_object_packed<S>(
        &mut self,
        store: &S,
        _packed: Option<&crate::packed::Buffer>,
    ) -> Result<ObjectId, peel::to_object::Error>
    where
        S: ReferenceStore + ?Sized,
    {
        match self.target {
            Target::Object(id) => Ok(id),
            Target::Symbolic(_) => {
                let mut seen = BTreeSet::new();
                while let Some(next) = self.follow(store) {
                    let next = next?;
                    if seen.contains(&next.name) {
                        return Err(peel::to_object::Error::Cycle {
                            start_absolute: self.name.to_path().to_owned(),
                        });
                    }
                    *self = next;
                    seen.insert(self.name.clone());
                    const MAX_REF_DEPTH: usize = 5;
                    if seen.len() == MAX_REF_DEPTH {
                        return Err(peel::to_object::Error::DepthLimitExceeded {
                            max_depth: MAX_REF_DEPTH,
                        });
                    }
                }
                Ok(self.target.try_id().expect("peeled ref").to_owned())
            }
        }
    }

    /// Follow this symbolic reference one level and return the referenced ref.
    pub fn follow<S>(&self, store: &S) -> Option<Result<Reference, crate::store::find::existing::Error>>
    where
        S: ReferenceStore + ?Sized,
    {
        match &self.target {
            Target::Object(_) => None,
            Target::Symbolic(full_name) => match store.try_find_reference(full_name.as_ref()) {
                Ok(Some(next)) => Some(Ok(next)),
                Ok(None) => Some(Err(crate::store::find::existing::Error::NotFound {
                    name: full_name.as_ref().as_partial_name().to_owned(),
                })),
                Err(err) => Some(Err(crate::store::find::existing::Error::Find(err))),
            },
        }
    }
}
