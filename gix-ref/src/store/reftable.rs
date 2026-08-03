use std::{
    cmp::Ordering,
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
        match self.iter(None).ok()?.next() {
            None => Some(true),
            Some(Ok(_)) => Some(false),
            Some(Err(_)) => None,
        }
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
        record_to_reference(record.into(), routed.visible).map(Some)
    }

    /// Iterate references with an optional prefix, in lexicographical order of their name.
    ///
    /// The returned iterator reads records from its own snapshot of the reftable stack as it goes,
    /// so it does not block or get blocked by other users of this store.
    pub fn iter(&self, prefix: Option<&[u8]>) -> Result<Iter, Error> {
        self.iter_impl(prefix, None)
    }

    /// As [`iter(…)`](Self::iter()), but starts iteration at the first reference whose name is
    /// equal to `from` or greater than it lexicographically, skipping all references before it
    /// in `log(n)` time.
    ///
    /// This is useful to resume an iteration, for instance to serve sorted references page by page.
    pub fn iter_from(&self, prefix: Option<&[u8]>, from: &[u8]) -> Result<Iter, Error> {
        self.iter_impl(prefix, Some(from))
    }

    fn iter_impl(&self, prefix: Option<&[u8]>, from: Option<&[u8]>) -> Result<Iter, Error> {
        const REFS: &[u8] = b"refs/";
        // Reduce the `refs/` requirement and the caller prefix to a single name prefix which
        // delimits all matching records, the more specific of the two. If neither contains the
        // other, no name can match both.
        let bound: Option<&[u8]> = match prefix {
            None => Some(REFS),
            Some(prefix) if prefix.starts_with(REFS) => Some(prefix),
            Some(prefix) if REFS.starts_with(prefix) => Some(REFS),
            Some(_) => None,
        };
        let Some(bound) = bound else {
            return Ok(Iter::empty());
        };
        let start = match from {
            Some(from) if from > bound => from,
            _ => bound,
        };
        if !start.starts_with(REFS) {
            // `start` is greater than `bound` yet not within `refs/`, so it is greater than
            // every name that could match `bound`.
            return Ok(Iter::empty());
        }
        let stored_bound = self.namespaced_name(bound.as_bstr());
        let stored_start = self.namespaced_name(start.as_bstr());

        let open_seeked = |dir: &Path| -> Result<Cursor, Error> {
            let stack = gix_reftable::Stack::open(&dir.join("reftable"), self.options)?;
            let mut iter = stack.into_references()?;
            iter.seek(&stored_start)?;
            Ok(Cursor {
                iter,
                peeked: None,
                done: false,
            })
        };
        let main = open_seeked(self.common_dir_resolved())?;
        let worktree = self
            .common_dir
            .as_ref()
            .map(|_| open_seeked(&self.git_dir))
            .transpose()?;
        Ok(Iter {
            main: Some(main),
            worktree,
            stored_bound,
            namespace: self.namespace.clone(),
            exclude_worktree_private_from_main: self.common_dir.is_some(),
        })
    }

    /// Iterate pseudo references only.
    pub fn pseudo(&self) -> Result<Vec<Reference>, Error> {
        let mut unnamespaced = self.clone();
        unnamespaced.namespace = None;
        let mut out = Vec::new();
        for (_, record) in unnamespaced.merged_records()? {
            if crate::name::is_pseudo_ref(record.name.as_bstr()) {
                let name = record.name.clone();
                out.push(record_to_reference(record, name)?);
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
}

fn record_to_reference(record: StoredRefRecord, visible_name: BString) -> Result<Reference, Error> {
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

fn visible_name(namespace: Option<&Namespace>, stored_name: &BStr) -> Option<BString> {
    match namespace {
        Some(namespace) => stored_name
            .strip_prefix(namespace.as_bstr().as_bytes())
            .map(BString::from),
        None => Some(stored_name.to_owned()),
    }
}

/// An iterator over the references of a reftable [`Store`], sorted by name.
///
/// It merges the shared main stack with the current worktree's private stack if present, reading
/// records from its own snapshot of each stack as it goes.
pub struct Iter {
    main: Option<Cursor>,
    worktree: Option<Cursor>,
    /// The stored-name prefix every record has to match for iteration to continue.
    stored_bound: BString,
    namespace: Option<Namespace>,
    exclude_worktree_private_from_main: bool,
}

struct Cursor {
    iter: gix_reftable::OwnedRefIter,
    peeked: Option<StoredRefRecord>,
    done: bool,
}

impl Cursor {
    /// Assure `peeked` holds the next record within `bound`, or mark this cursor as done.
    fn fill(&mut self, bound: &BStr) -> Result<(), Error> {
        if self.done || self.peeked.is_some() {
            return Ok(());
        }
        match self.iter.next_record()? {
            Some(record) if record.name.starts_with(bound.as_bytes()) => {
                self.peeked = Some(record.into());
            }
            // Names are iterated in ascending order, so nothing past the bound can match anymore.
            Some(_) | None => self.done = true,
        }
        Ok(())
    }

    fn peeked_name(&self) -> Option<&BStr> {
        self.peeked.as_ref().map(|record| record.name.as_bstr())
    }
}

impl Iter {
    fn empty() -> Self {
        Iter {
            main: None,
            worktree: None,
            stored_bound: BString::default(),
            namespace: None,
            exclude_worktree_private_from_main: false,
        }
    }

    /// Return the next merged record within bounds, preferring the worktree stack on name
    /// collisions just like a worktree stack record would shadow one in the shared stack.
    fn next_record(&mut self) -> Result<Option<(StoredRefRecord, bool)>, Error> {
        if let Some(main) = self.main.as_mut() {
            main.fill(self.stored_bound.as_bstr())?;
        }
        if let Some(worktree) = self.worktree.as_mut() {
            worktree.fill(self.stored_bound.as_bstr())?;
        }
        let main_name = self.main.as_ref().and_then(Cursor::peeked_name);
        let worktree_name = self.worktree.as_ref().and_then(Cursor::peeked_name);
        let (from_main, shadowed_main) = match (main_name, worktree_name) {
            (None, None) => return Ok(None),
            (Some(_), None) => (true, false),
            (None, Some(_)) => (false, false),
            (Some(main), Some(worktree)) => match main.cmp(worktree) {
                Ordering::Less => (true, false),
                Ordering::Greater => (false, false),
                Ordering::Equal => (false, true),
            },
        };
        if shadowed_main {
            self.main
                .as_mut()
                .expect("main cursor has a record to be shadowed")
                .peeked
                .take();
        }
        let cursor = if from_main { &mut self.main } else { &mut self.worktree };
        let record = cursor
            .as_mut()
            .expect("cursor with the smallest name has a record")
            .peeked
            .take()
            .expect("cursor with the smallest name has a record");
        Ok(Some((record, from_main)))
    }
}

impl Iterator for Iter {
    type Item = Result<Reference, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (record, from_main) = match self.next_record() {
                Ok(Some(record)) => record,
                Ok(None) => return None,
                Err(err) => {
                    // Don't yield records after an error to avoid unnoticed gaps in the stream.
                    self.main = None;
                    self.worktree = None;
                    return Some(Err(err));
                }
            };
            // Tombstones hide older records, they are not listable references.
            if matches!(record.value, StoredRefValue::Deletion) {
                continue;
            }
            let Some(visible) = visible_name(self.namespace.as_ref(), record.name.as_bstr()) else {
                continue;
            };
            if from_main
                && self.exclude_worktree_private_from_main
                && FullNameRef::new_unchecked(visible.as_bstr())
                    .category()
                    .is_some_and(|category| category.is_worktree_private())
            {
                continue;
            }
            return Some(record_to_reference(record, visible));
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
