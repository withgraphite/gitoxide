use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gix_object::bstr::{BStr, BString, ByteSlice};

use crate::{FullName, FullNameRef, Namespace, PartialNameRef, Reference, Target, log::Line, store::WriteReflog};

/// Options controlling C reftable writes and compaction.
pub use gix_reftable::Options as WriteOptions;

/// The error returned by reftable-backed reference store operations.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Reftable(#[from] gix_reftable::Error),
    #[error(transparent)]
    Name(#[from] crate::name::Error),
    #[error("Reftable record had an invalid timezone offset: {0}")]
    InvalidTimezone(i16),
    #[error("Reftable record timestamp does not fit into an i64: {0}")]
    InvalidTimestamp(u64),
    #[error("Reftable iterator returned a deleted reference")]
    DeletedReference,
}

impl From<std::convert::Infallible> for Error {
    fn from(_: std::convert::Infallible) -> Self {
        unreachable!("infallible conversion cannot fail")
    }
}

#[derive(Debug)]
struct StackSet {
    common_dir: PathBuf,
    main: gix_reftable::Stack,
    current_worktree: Option<gix_reftable::Stack>,
    linked_worktrees: BTreeMap<BString, gix_reftable::Stack>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum StackKey {
    Main,
    CurrentWorktree,
    LinkedWorktree(BString),
}

#[derive(Debug)]
struct RoutedName {
    stack: StackKey,
    logical: BString,
    stored: BString,
    visible: BString,
}

struct StoredRefRecord {
    name: BString,
    value: StoredRefValue,
}

enum StoredRefValue {
    Deletion,
    Object(gix_hash::ObjectId),
    Peeled {
        object: gix_hash::ObjectId,
        peeled: gix_hash::ObjectId,
    },
    Symbolic(BString),
}

impl From<gix_reftable::RefRecord<'_>> for StoredRefRecord {
    fn from(record: gix_reftable::RefRecord<'_>) -> Self {
        StoredRefRecord {
            name: record.name.to_owned(),
            value: match record.value {
                gix_reftable::RefValue::Deletion => StoredRefValue::Deletion,
                gix_reftable::RefValue::Object(object) => StoredRefValue::Object(object.to_owned()),
                gix_reftable::RefValue::Peeled { object, peeled } => StoredRefValue::Peeled {
                    object: object.to_owned(),
                    peeled: peeled.to_owned(),
                },
                gix_reftable::RefValue::Symbolic(target) => StoredRefValue::Symbolic(target.to_owned()),
            },
        }
    }
}

mod transaction;
pub use transaction::{Error as TransactionError, Transaction};

impl StackSet {
    fn stack(&mut self, key: &StackKey, options: gix_reftable::Options) -> Result<&mut gix_reftable::Stack, Error> {
        match key {
            StackKey::Main => Ok(&mut self.main),
            StackKey::CurrentWorktree => Ok(self.current_worktree.as_mut().unwrap_or(&mut self.main)),
            StackKey::LinkedWorktree(name) => {
                if !self.linked_worktrees.contains_key(name) {
                    let path = self
                        .common_dir
                        .join("worktrees")
                        .join(gix_path::from_bstr(name))
                        .join("reftable");
                    let stack = gix_reftable::Stack::open(&path, options)?;
                    self.linked_worktrees.insert(name.clone(), stack);
                }
                Ok(self
                    .linked_worktrees
                    .get_mut(name)
                    .expect("linked worktree stack was inserted"))
            }
        }
    }
}

/// A reference store backed by Git's reftable format.
#[derive(Debug, Clone)]
pub struct Store {
    git_dir: PathBuf,
    common_dir: Option<PathBuf>,
    stacks: gix_features::threading::OwnShared<gix_features::threading::Mutable<StackSet>>,
    options: gix_reftable::Options,
    /// The way reflogs are written.
    pub write_reflog: WriteReflog,
    /// The namespace to use for reads and writes.
    pub namespace: Option<Namespace>,
    /// The equivalent of `core.precomposeUnicode`.
    pub precompose_unicode: bool,
}

impl Store {
    /// Open a reftable-backed store at `git_dir`.
    pub fn at(git_dir: PathBuf, opts: crate::store::init::Options) -> Result<Self, Error> {
        Self::open(git_dir, None, opts)
    }

    /// Open a reftable-backed store for a linked worktree.
    pub fn for_linked_worktree(
        git_dir: PathBuf,
        common_dir: PathBuf,
        opts: crate::store::init::Options,
    ) -> Result<Self, Error> {
        Self::open(git_dir, Some(common_dir), opts)
    }

    fn open(git_dir: PathBuf, common_dir: Option<PathBuf>, opts: crate::store::init::Options) -> Result<Self, Error> {
        let options = gix_reftable::Options {
            hash: opts.object_hash,
            ..opts.reftable
        };
        let common_dir_resolved = common_dir.as_deref().unwrap_or(&git_dir).to_owned();
        let main = gix_reftable::Stack::open(&common_dir_resolved.join("reftable"), options)?;
        let current_worktree = common_dir
            .as_ref()
            .map(|_| git_dir.join("reftable"))
            .map(|path| gix_reftable::Stack::open(&path, options))
            .transpose()?;
        Ok(Store {
            git_dir,
            common_dir,
            stacks: gix_features::threading::OwnShared::new(gix_features::threading::Mutable::new(StackSet {
                common_dir: common_dir_resolved,
                main,
                current_worktree,
                linked_worktrees: BTreeMap::new(),
            })),
            options,
            write_reflog: opts.write_reflog,
            namespace: None,
            precompose_unicode: opts.precompose_unicode,
        })
    }

    /// Return the `.git` directory at which references are loaded.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Return the common directory if this is for a linked worktree.
    pub fn common_dir(&self) -> Option<&Path> {
        self.common_dir.as_deref()
    }

    /// Return the common directory or the git directory.
    pub fn common_dir_resolved(&self) -> &Path {
        self.common_dir.as_deref().unwrap_or(&self.git_dir)
    }

    /// Return whether the store has no reference other than an unborn default ref.
    pub fn is_pristine(&self, default_ref: &crate::FullNameRef) -> Option<bool> {
        let head = FullNameRef::new_unchecked(b"HEAD".as_bstr());
        let head = self.try_find_full_name(head).ok()??;
        if head.target.try_name() != Some(default_ref) {
            return Some(false);
        }
        Some(self.iter(None).ok()?.is_empty())
    }

    /// Try to find a reference by partial name.
    pub fn try_find<'a, Name, E>(&self, partial: Name) -> Result<Option<Reference>, Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E>,
        Error: From<E>,
    {
        let partial = partial.try_into()?;
        let mut buf = BString::default();
        for consider_pseudo_ref in [true, false] {
            if !consider_pseudo_ref && !crate::name::is_pseudo_ref(partial.as_bstr()) {
                break;
            }
            for inbetween in &["", "tags", "heads", "remotes"] {
                let full = partial.construct_full_name_ref(inbetween, &mut buf, consider_pseudo_ref);
                if let Some(reference) = self.try_find_full_name(full)? {
                    return Ok(Some(reference));
                }
            }
        }
        if partial.as_bstr() != "HEAD" {
            let remote_head = partial.to_owned().join("HEAD".into())?;
            let full = remote_head.as_ref().construct_full_name_ref("remotes", &mut buf, true);
            return self.try_find_full_name(full);
        }
        Ok(None)
    }

    /// Similar to [`try_find()`][Self::try_find], but a missing reference is treated as error.
    pub fn find<'a, Name, E>(&self, partial: Name) -> Result<Reference, crate::store::find::existing::Error>
    where
        Name: TryInto<&'a PartialNameRef, Error = E> + Clone,
        crate::store::find::Error: From<E>,
    {
        let partial_name = partial.clone().try_into().map_err(crate::store::find::Error::from)?;
        let partial = partial.try_into().map_err(crate::store::find::Error::from)?;
        self.try_find(partial)
            .map_err(crate::store::find::Error::Reftable)?
            .ok_or_else(|| crate::store::find::existing::Error::NotFound {
                name: partial_name.to_owned(),
            })
    }

    fn try_find_full_name(&self, name: &FullNameRef) -> Result<Option<Reference>, Error> {
        let routed = self.route_name(name);
        let mut stacks = gix_features::threading::lock(&self.stacks);
        let stack = stacks.stack(&routed.stack, self.options)?;
        stack.reload()?;
        let mut iter = stack.references()?;
        iter.seek(&routed.stored)?;
        let Some(record) = iter.next_record()? else {
            return Ok(None);
        };
        if record.name != routed.stored {
            return Ok(None);
        }
        self.record_to_reference(record.into(), routed.visible).map(Some)
    }

    /// Iterate references with an optional prefix.
    pub fn iter(&self, prefix: Option<&[u8]>) -> Result<Vec<Reference>, Error> {
        let records = self.merged_records()?;
        let mut out = Vec::with_capacity(records.len());
        for (_, record) in records {
            let Some(visible) = self.visible_name(record.name.as_bstr()) else {
                continue;
            };
            if !visible.starts_with_str("refs/") {
                continue;
            }
            if prefix.is_some_and(|prefix| !visible.starts_with(prefix)) {
                continue;
            }
            if matches!(record.value, StoredRefValue::Deletion) {
                continue;
            }
            out.push(self.record_to_reference(record, visible)?);
        }
        Ok(out)
    }

    /// Iterate pseudo references only.
    pub fn pseudo(&self) -> Result<Vec<Reference>, Error> {
        let mut unnamespaced = self.clone();
        unnamespaced.namespace = None;
        let mut out = Vec::new();
        for (_, record) in unnamespaced.merged_records()? {
            if crate::name::is_pseudo_ref(record.name.as_bstr()) {
                let name = record.name.clone();
                out.push(unnamespaced.record_to_reference(record, name)?);
            }
        }
        Ok(out)
    }

    /// Return true if a reflog exists for `name`.
    pub fn reflog_exists(&self, name: &FullNameRef) -> Result<bool, Error> {
        let routed = self.route_name(name);
        let mut stacks = gix_features::threading::lock(&self.stacks);
        let stack = stacks.stack(&routed.stack, self.options)?;
        stack.reload()?;
        let mut iter = stack.logs()?;
        iter.seek(&routed.stored)?;
        Ok(iter.next_record()?.is_some_and(|record| record.name == routed.stored))
    }

    /// Return reflog lines for `name`.
    pub fn reflog(&self, name: &FullNameRef, newest_first: bool) -> Result<Option<Vec<Line>>, Error> {
        let routed = self.route_name(name);
        let mut stacks = gix_features::threading::lock(&self.stacks);
        let stack = stacks.stack(&routed.stack, self.options)?;
        stack.reload()?;
        let mut iter = stack.logs()?;
        iter.seek(&routed.stored)?;
        let mut lines = Vec::new();
        while let Some(record) = iter.next_record()? {
            if record.name != routed.stored {
                break;
            }
            let gix_reftable::LogValue::Update {
                old,
                new,
                name,
                email,
                time,
                tz_offset,
                message,
            } = record.value
            else {
                continue;
            };
            if old.as_bytes().iter().all(|byte| *byte == 0) && new.as_bytes().iter().all(|byte| *byte == 0) {
                continue;
            }
            let message = message
                .strip_suffix(b"\n")
                .map_or(message, |message| message.as_bstr())
                .to_owned();
            lines.push(Line {
                previous_oid: old.to_owned(),
                new_oid: new.to_owned(),
                signature: gix_actor::Signature {
                    name: name.to_owned(),
                    email: email.to_owned(),
                    time: gix_actor::date::Time {
                        seconds: time.try_into().map_err(|_| Error::InvalidTimestamp(time))?,
                        offset: timezone_to_seconds(tz_offset)?,
                    },
                },
                message,
            });
        }
        if lines.is_empty() {
            Ok(None)
        } else {
            if !newest_first {
                lines.reverse();
            }
            Ok(Some(lines))
        }
    }

    fn merged_records(&self) -> Result<BTreeMap<BString, StoredRefRecord>, Error> {
        let mut stacks = gix_features::threading::lock(&self.stacks);
        let mut records = BTreeMap::new();
        collect_refs(
            &mut stacks.main,
            &mut records,
            self.common_dir.is_some(),
            self.namespace.as_ref().map(Namespace::as_bstr),
        )?;
        if let Some(worktree) = stacks.current_worktree.as_mut() {
            collect_refs(worktree, &mut records, false, None)?;
        }
        Ok(records)
    }

    fn route_name(&self, name: &FullNameRef) -> RoutedName {
        let visible = name.as_bstr().to_owned();
        let (stack, logical) = match name.category_and_short_name() {
            None => (StackKey::Main, visible.clone()),
            Some((category, short)) => {
                use crate::Category::*;
                match category {
                    Tag | LocalBranch | RemoteBranch | Note => (StackKey::Main, visible.clone()),
                    MainRef | MainPseudoRef => (StackKey::Main, short.to_owned()),
                    LinkedPseudoRef { name } => (StackKey::LinkedWorktree(name.to_owned()), short.to_owned()),
                    LinkedRef { name } => {
                        let short_name = FullNameRef::new_unchecked(short);
                        if short_name
                            .category()
                            .is_some_and(|category| category.is_worktree_private())
                        {
                            (StackKey::LinkedWorktree(name.to_owned()), short.to_owned())
                        } else {
                            (StackKey::Main, short.to_owned())
                        }
                    }
                    PseudoRef | Bisect | Rewritten | WorktreePrivate => (StackKey::CurrentWorktree, visible.clone()),
                }
            }
        };
        let stored = self.namespaced_name(logical.as_bstr());
        RoutedName {
            stack,
            logical,
            stored,
            visible,
        }
    }

    fn record_to_reference(&self, record: StoredRefRecord, visible_name: BString) -> Result<Reference, Error> {
        let name = FullName::try_from(visible_name)?;
        let reference = match record.value {
            StoredRefValue::Deletion => {
                return Err(Error::DeletedReference);
            }
            StoredRefValue::Object(value) => Reference {
                name,
                target: Target::Object(value),
                peeled: None,
            },
            StoredRefValue::Peeled { object, peeled } => Reference {
                name,
                target: Target::Object(object),
                peeled: Some(peeled),
            },
            StoredRefValue::Symbolic(target) => Reference {
                name,
                target: Target::Symbolic(FullName::try_from(target)?),
                peeled: None,
            },
        };
        Ok(reference)
    }

    fn namespaced_name(&self, name: &BStr) -> BString {
        if let Some(namespace) = &self.namespace
            && name.starts_with_str("refs/")
        {
            let mut out = namespace.as_bstr().to_owned();
            out.extend_from_slice(name);
            return out;
        }
        name.to_owned()
    }

    fn visible_name(&self, stored_name: &BStr) -> Option<BString> {
        match &self.namespace {
            Some(namespace) => stored_name
                .strip_prefix(namespace.as_bstr().as_bytes())
                .map(BString::from),
            None => Some(stored_name.to_owned()),
        }
    }
}

fn collect_refs(
    stack: &mut gix_reftable::Stack,
    records: &mut BTreeMap<BString, StoredRefRecord>,
    shared_only: bool,
    namespace: Option<&BStr>,
) -> Result<(), Error> {
    stack.reload()?;
    let mut iter = stack.references()?;
    iter.seek(b"")?;
    while let Some(record) = iter.next_record()? {
        let classified_name = namespace
            .and_then(|namespace| record.name.strip_prefix(namespace.as_bytes()))
            .map_or(record.name, |name| name.as_bstr());
        if shared_only
            && FullNameRef::new_unchecked(classified_name)
                .category()
                .is_some_and(|category| category.is_worktree_private())
        {
            continue;
        }
        let record = StoredRefRecord::from(record);
        records.insert(record.name.clone(), record);
    }
    Ok(())
}

fn timezone_to_seconds(offset: i16) -> Result<i32, Error> {
    let sign = if offset < 0 { -1 } else { 1 };
    let absolute = i32::from(offset).abs();
    let hours = absolute / 100;
    let minutes = absolute % 100;
    if minutes >= 60 {
        return Err(Error::InvalidTimezone(offset));
    }
    Ok(sign * (hours * 60 * 60 + minutes * 60))
}
