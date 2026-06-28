use std::collections::{BTreeMap, BTreeSet};

use gix_object::bstr::{BString, ByteSlice};

use super::{RoutedName, StackKey, Store};
use crate::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefEditsExt, RefLog},
};

/// A reftable reference transaction.
pub struct Transaction<'store, 'objects> {
    store: &'store Store,
    objects: Option<&'objects dyn gix_object::Find>,
    edits: Option<Vec<Edit>>,
    prepared: Option<BTreeMap<StackKey, PreparedStack>>,
}

struct Edit {
    update: RefEdit,
    route: Option<RoutedName>,
    parent_index: Option<usize>,
    requested_deref: bool,
    head_guard: Option<FullName>,
    head_source_index: Option<usize>,
    leaf_referent_previous_oid: Option<gix_hash::ObjectId>,
    effective: bool,
}

impl std::borrow::Borrow<RefEdit> for Edit {
    fn borrow(&self) -> &RefEdit {
        &self.update
    }
}

impl std::borrow::BorrowMut<RefEdit> for Edit {
    fn borrow_mut(&mut self) -> &mut RefEdit {
        &mut self.update
    }
}

struct PreparedStack {
    addition: gix_reftable::Addition,
    edit_indices: Vec<usize>,
}

#[derive(Default)]
struct Table {
    refs: Vec<WriteRefRecord>,
    logs: Vec<WriteLogRecord>,
}

struct WriteRefRecord {
    name: BString,
    update_index: u64,
    value: WriteRefValue,
}

enum WriteRefValue {
    Deletion,
    Object(gix_hash::ObjectId),
    Peeled {
        object: gix_hash::ObjectId,
        peeled: gix_hash::ObjectId,
    },
    Symbolic(BString),
}

impl WriteRefRecord {
    fn as_ref(&self) -> gix_reftable::RefRecord<'_> {
        gix_reftable::RefRecord {
            name: self.name.as_bstr(),
            update_index: self.update_index,
            value: match &self.value {
                WriteRefValue::Deletion => gix_reftable::RefValue::Deletion,
                WriteRefValue::Object(object) => gix_reftable::RefValue::Object(object.as_ref()),
                WriteRefValue::Peeled { object, peeled } => gix_reftable::RefValue::Peeled {
                    object: object.as_ref(),
                    peeled: peeled.as_ref(),
                },
                WriteRefValue::Symbolic(target) => gix_reftable::RefValue::Symbolic(target.as_bstr()),
            },
        }
    }
}

struct WriteLogRecord {
    name: BString,
    update_index: u64,
    value: WriteLogValue,
}

enum WriteLogValue {
    Deletion,
    Update {
        old: gix_hash::ObjectId,
        new: gix_hash::ObjectId,
        name: BString,
        email: BString,
        time: u64,
        tz_offset: i16,
        message: BString,
    },
}

impl WriteLogRecord {
    fn as_ref(&self) -> gix_reftable::LogRecord<'_> {
        gix_reftable::LogRecord {
            name: self.name.as_bstr(),
            update_index: self.update_index,
            value: match &self.value {
                WriteLogValue::Deletion => gix_reftable::LogValue::Deletion,
                WriteLogValue::Update {
                    old,
                    new,
                    name,
                    email,
                    time,
                    tz_offset,
                    message,
                } => gix_reftable::LogValue::Update {
                    old: old.as_ref(),
                    new: new.as_ref(),
                    name: name.as_bstr(),
                    email: email.as_bstr(),
                    time: *time,
                    tz_offset: *tz_offset,
                    message: message.as_bstr(),
                },
            },
        }
    }
}

/// The error returned while preparing or committing a reftable transaction.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] super::Error),
    #[error(transparent)]
    Reftable(#[from] gix_reftable::Error),
    #[error(transparent)]
    Name(#[from] crate::name::Error),
    #[error("Edit preprocessing failed")]
    Preprocessing(#[source] std::io::Error),
    #[error("Could not create a reftable stack directory")]
    CreateDirectory(#[source] std::io::Error),
    #[error("MustNotExist is invalid for deletion of {name:?}")]
    InvalidDeleteConstraint { name: BString },
    #[error("Reference {name:?} must exist")]
    MustExist { name: BString },
    #[error("Reference {name:?} must not exist, but contains {actual}")]
    MustNotExist { name: BString, actual: Target },
    #[error("Reference {name:?} contains {actual}, expected {expected}")]
    ReferenceOutOfDate {
        name: BString,
        expected: Target,
        actual: Target,
    },
    #[error("Reference {name:?} became symbolic while preparing a dereferencing update")]
    DereferenceChanged { name: BString },
    #[error("HEAD changed while preparing its implicit reflog update")]
    HeadChanged,
    #[error("Reference names {first:?} and {second:?} conflict")]
    NameConflict { first: BString, second: BString },
    #[error("Multiple edits route to the same reftable key {stored:?}")]
    DuplicateRoutedName { stored: BString },
    #[error("A reflog update requires a committer")]
    MissingCommitter,
    #[error("Committer timestamp must not be negative")]
    NegativeTimestamp,
    #[error("Committer timestamp is invalid")]
    InvalidCommitterTime(#[source] gix_actor::date::Error),
    #[error("Committer timezone offset cannot be represented as ±HHMM: {0}")]
    InvalidTimezone(i32),
    #[error("Reflog messages must be one NUL-free line")]
    InvalidLogMessage,
}

impl Store {
    /// Open a transaction that can edit this reftable store.
    pub fn transaction(&self) -> Transaction<'_, '_> {
        Transaction {
            store: self,
            objects: None,
            edits: None,
            prepared: None,
        }
    }
}

impl<'store> Transaction<'store, '_> {
    /// Supply an object database so direct tag references can store peeled values.
    pub fn objects<'new>(self, objects: &'new dyn gix_object::Find) -> Transaction<'store, 'new> {
        Transaction {
            store: self.store,
            objects: Some(objects),
            edits: self.edits,
            prepared: self.prepared,
        }
    }

    /// Prepare edits and lock every affected reftable stack.
    pub fn prepare(
        mut self,
        edits: impl IntoIterator<Item = RefEdit>,
        _fail: gix_lock::acquire::Fail,
    ) -> Result<Self, Error> {
        assert!(self.edits.is_none(), "BUG: transaction cannot be prepared twice");
        let store = self.store;
        let mut edits: Vec<_> = edits
            .into_iter()
            .map(|update| {
                let requested_deref = update.deref;
                Edit {
                    update,
                    route: None,
                    parent_index: None,
                    requested_deref,
                    head_guard: None,
                    head_source_index: None,
                    leaf_referent_previous_oid: None,
                    effective: false,
                }
            })
            .collect();
        edits
            .pre_process(
                &mut |name| store.try_find(name).ok().flatten().map(|reference| reference.target),
                &mut |parent_index, update| {
                    let requested_deref = update.deref;
                    Edit {
                        update,
                        route: None,
                        parent_index: Some(parent_index),
                        requested_deref,
                        head_guard: None,
                        head_source_index: None,
                        leaf_referent_previous_oid: None,
                        effective: false,
                    }
                },
            )
            .map_err(Error::Preprocessing)?;
        inject_head_log(store, &mut edits)?;

        let mut indices_by_stack = BTreeMap::<StackKey, Vec<usize>>::new();
        let mut routed_keys = BTreeSet::new();
        for (index, edit) in edits.iter_mut().enumerate() {
            edit.update.deref = false;
            let mut route = store.route_name(edit.update.name.as_ref());
            if matches!(route.stack, StackKey::CurrentWorktree) && store.common_dir.is_none() {
                route.stack = StackKey::Main;
            }
            if !routed_keys.insert((route.stack.clone(), route.stored.clone())) {
                return Err(Error::DuplicateRoutedName { stored: route.stored });
            }
            indices_by_stack.entry(route.stack.clone()).or_default().push(index);
            edit.route = Some(route);
        }

        let mut prepared = BTreeMap::new();
        for (key, edit_indices) in indices_by_stack {
            let path = stack_path(store, &key);
            std::fs::create_dir_all(&path).map_err(Error::CreateDirectory)?;
            let addition = gix_reftable::Stack::open(&path, store.options)?.into_addition()?;
            prepared.insert(key, PreparedStack { addition, edit_indices });
        }

        validate_head_guards(&mut prepared, &edits)?;
        validate_unsplit_dereferences(&mut prepared, &edits)?;
        validate_symbolic_splits(&mut prepared, &edits)?;
        for stack in prepared.values_mut() {
            for index in stack.edit_indices.iter().copied() {
                let existing = {
                    let route = edits[index].route.as_ref().expect("route assigned");
                    read_target(&mut stack.addition, &route.stored)?
                };
                validate_edit(&mut edits[index], existing)?;
            }
        }
        propagate_leaf_values(&mut edits);
        propagate_head_log_values(&mut edits);
        validate_name_conflicts(&mut prepared, &edits)?;

        self.edits = Some(edits);
        self.prepared = Some(prepared);
        Ok(self)
    }

    /// Commit a prepared transaction and return the normalized edits.
    pub fn commit(mut self, committer: Option<gix_actor::SignatureRef<'_>>) -> Result<Vec<RefEdit>, Error> {
        let mut edits = self
            .edits
            .take()
            .expect("BUG: transaction must be prepared before commit");
        let mut prepared = self
            .prepared
            .take()
            .expect("BUG: transaction must be prepared before commit");
        let mut tables = BTreeMap::new();

        for (key, stack) in &mut prepared {
            let table = build_table(self.store, self.objects, stack, &edits, committer)?;
            tables.insert(key.clone(), table);
        }

        let mut staged = BTreeSet::new();
        for (key, stack) in &mut prepared {
            let table = tables.remove(key).expect("table exists for each prepared stack");
            if table.refs.is_empty() && table.logs.is_empty() {
                continue;
            }
            let update_index = stack.addition.next_update_index();
            stack.addition.add_table(move |writer| {
                writer.set_limits(update_index, update_index);
                for record in &table.refs {
                    writer.add_ref(record.as_ref())?;
                }
                for record in &table.logs {
                    writer.add_log(record.as_ref())?;
                }
                Ok(())
            })?;
            staged.insert(key.clone());
        }
        for (key, stack) in prepared {
            if staged.contains(&key) {
                drop(stack.addition.commit()?);
            }
        }

        Ok(edits.drain(..).map(|edit| edit.update).collect())
    }
}

fn stack_path(store: &Store, key: &StackKey) -> std::path::PathBuf {
    match key {
        StackKey::Main => store.common_dir_resolved().join("reftable"),
        StackKey::CurrentWorktree => store.git_dir.join("reftable"),
        StackKey::LinkedWorktree(name) => store
            .common_dir_resolved()
            .join("worktrees")
            .join(gix_path::from_bstr(name))
            .join("reftable"),
    }
}

fn inject_head_log(store: &Store, edits: &mut Vec<Edit>) -> Result<(), Error> {
    let head_name = crate::FullNameRef::new_unchecked(b"HEAD".as_bstr());
    let Some(head) = store.try_find(head_name)? else {
        return Ok(());
    };
    let Some(referent) = head.target.try_name().map(ToOwned::to_owned) else {
        return Ok(());
    };
    if edits.iter().any(|edit| edit.update.name.as_bstr() == "HEAD") {
        return Ok(());
    }
    let referent_route = store.route_name(referent.as_ref());
    let Some(source_index) = edits.iter().position(|edit| {
        let route = store.route_name(edit.update.name.as_ref());
        route.stack == referent_route.stack
            && route.stored == referent_route.stored
            && matches!(
                edit.update.change,
                Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        ..
                    },
                    ..
                } | Change::Delete {
                    log: RefLog::AndReference,
                    ..
                }
            )
    }) else {
        return Ok(());
    };
    let (log, new) = match &edits[source_index].update.change {
        Change::Update { log, new, .. } => (log.clone(), new.clone()),
        Change::Delete { .. } => {
            let Some(old) = store
                .try_find(referent.as_ref())?
                .and_then(|reference| reference.target.try_id().map(ToOwned::to_owned))
            else {
                return Ok(());
            };
            (
                LogChange {
                    mode: RefLog::Only,
                    force_create_reflog: false,
                    message: BString::default(),
                },
                Target::Object(old.kind().null()),
            )
        }
    };
    edits.push(Edit {
        update: RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::Only,
                    force_create_reflog: log.force_create_reflog,
                    message: log.message,
                },
                expected: PreviousValue::Any,
                new,
            },
            name: "HEAD".try_into()?,
            deref: false,
        },
        route: None,
        parent_index: None,
        requested_deref: false,
        head_guard: Some(referent),
        head_source_index: Some(source_index),
        leaf_referent_previous_oid: None,
        effective: false,
    });
    Ok(())
}

fn read_target(addition: &mut gix_reftable::Addition, name: &[u8]) -> Result<Option<Target>, Error> {
    let mut iter = addition.references()?;
    iter.seek(name)?;
    let Some(record) = iter.next_record()? else {
        return Ok(None);
    };
    if record.name.as_bytes() != name {
        return Ok(None);
    }
    match record.value {
        gix_reftable::RefValue::Deletion => Ok(None),
        gix_reftable::RefValue::Object(object) | gix_reftable::RefValue::Peeled { object, .. } => {
            Ok(Some(Target::Object(object.to_owned())))
        }
        gix_reftable::RefValue::Symbolic(target) => Ok(Some(Target::Symbolic(FullName::try_from(target)?))),
    }
}

fn validate_edit(edit: &mut Edit, existing: Option<Target>) -> Result<(), Error> {
    let name = edit.update.name.as_bstr().to_owned();
    match &mut edit.update.change {
        Change::Delete { expected, .. } => {
            match (&*expected, &existing) {
                (PreviousValue::MustNotExist, _) => {
                    return Err(Error::InvalidDeleteConstraint { name });
                }
                (PreviousValue::ExistingMustMatch(_) | PreviousValue::Any, None)
                | (PreviousValue::MustExist | PreviousValue::Any, Some(_)) => {}
                (PreviousValue::MustExist | PreviousValue::MustExistAndMatch(_), None) => {
                    return Err(Error::MustExist { name });
                }
                (
                    PreviousValue::MustExistAndMatch(expected) | PreviousValue::ExistingMustMatch(expected),
                    Some(actual),
                ) if expected != actual => {
                    return Err(Error::ReferenceOutOfDate {
                        name,
                        expected: expected.clone(),
                        actual: actual.clone(),
                    });
                }
                _ => {}
            }
            if let Some(existing) = existing {
                *expected = PreviousValue::MustExistAndMatch(existing);
            }
            edit.effective = true;
        }
        Change::Update { expected, new, log } => {
            match (&*expected, &existing) {
                (PreviousValue::Any, _)
                | (PreviousValue::MustExist, Some(_))
                | (PreviousValue::MustNotExist | PreviousValue::ExistingMustMatch(_), None) => {}
                (PreviousValue::MustExist | PreviousValue::MustExistAndMatch(_), None) => {
                    return Err(Error::MustExist { name });
                }
                (PreviousValue::MustNotExist, Some(actual)) if actual != new => {
                    return Err(Error::MustNotExist {
                        name,
                        actual: actual.clone(),
                    });
                }
                (
                    PreviousValue::MustExistAndMatch(expected) | PreviousValue::ExistingMustMatch(expected),
                    Some(actual),
                ) if expected != actual => {
                    return Err(Error::ReferenceOutOfDate {
                        name,
                        expected: expected.clone(),
                        actual: actual.clone(),
                    });
                }
                _ => {}
            }
            edit.effective =
                existing.as_ref() != Some(new) || matches!(new, Target::Symbolic(_)) || log.mode == RefLog::Only;
            if let Some(existing) = existing {
                *expected = PreviousValue::MustExistAndMatch(existing);
            }
        }
    }
    Ok(())
}

fn validate_head_guards(prepared: &mut BTreeMap<StackKey, PreparedStack>, edits: &[Edit]) -> Result<(), Error> {
    for edit in edits {
        let Some(referent) = &edit.head_guard else {
            continue;
        };
        let route = edit.route.as_ref().expect("route assigned");
        let stack = prepared.get_mut(&route.stack).expect("every routed stack is prepared");
        if read_target(&mut stack.addition, &route.stored)? != Some(Target::Symbolic(referent.clone())) {
            return Err(Error::HeadChanged);
        }
    }
    Ok(())
}

fn validate_unsplit_dereferences(
    prepared: &mut BTreeMap<StackKey, PreparedStack>,
    edits: &[Edit],
) -> Result<(), Error> {
    let parents: BTreeSet<_> = edits.iter().filter_map(|edit| edit.parent_index).collect();
    for (index, edit) in edits.iter().enumerate() {
        if !edit.requested_deref || parents.contains(&index) {
            continue;
        }
        let route = edit.route.as_ref().expect("route assigned");
        let stack = prepared.get_mut(&route.stack).expect("every routed stack is prepared");
        if matches!(
            read_target(&mut stack.addition, &route.stored)?,
            Some(Target::Symbolic(_))
        ) {
            return Err(Error::DereferenceChanged {
                name: edit.update.name.as_bstr().to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_symbolic_splits(prepared: &mut BTreeMap<StackKey, PreparedStack>, edits: &[Edit]) -> Result<(), Error> {
    for edit in edits {
        let Some(parent_index) = edit.parent_index else {
            continue;
        };
        let parent = &edits[parent_index];
        let route = parent.route.as_ref().expect("route assigned");
        let stack = prepared.get_mut(&route.stack).expect("every routed stack is prepared");
        let expected = Target::Symbolic(edit.update.name.clone());
        match read_target(&mut stack.addition, &route.stored)? {
            Some(actual) if actual == expected => {}
            Some(actual) => {
                return Err(Error::ReferenceOutOfDate {
                    name: parent.update.name.as_bstr().to_owned(),
                    expected,
                    actual,
                });
            }
            None => {
                return Err(Error::MustExist {
                    name: parent.update.name.as_bstr().to_owned(),
                });
            }
        }
    }
    Ok(())
}

fn propagate_leaf_values(edits: &mut [Edit]) {
    for index in 0..edits.len() {
        let previous = match edits[index].update.change.previous_value() {
            Some(crate::TargetRef::Object(id)) => Some(id.to_owned()),
            _ => None,
        };
        let mut parent = edits[index].parent_index;
        if let Some(previous) = previous {
            while let Some(parent_index) = parent {
                edits[parent_index].leaf_referent_previous_oid = Some(previous);
                parent = edits[parent_index].parent_index;
            }
        }
    }
}

fn propagate_head_log_values(edits: &mut [Edit]) {
    for index in 0..edits.len() {
        let Some(source_index) = edits[index].head_source_index else {
            continue;
        };
        if let Some(crate::TargetRef::Object(previous)) = edits[source_index].update.change.previous_value() {
            edits[index].leaf_referent_previous_oid = Some(previous.to_owned());
        }
    }
}

fn validate_name_conflicts(prepared: &mut BTreeMap<StackKey, PreparedStack>, edits: &[Edit]) -> Result<(), Error> {
    for stack in prepared.values_mut() {
        let mut names = BTreeSet::new();
        {
            let mut iter = stack.addition.references()?;
            iter.seek(b"")?;
            while let Some(record) = iter.next_record()? {
                names.insert(record.name.to_owned());
            }
        }
        for index in stack.edit_indices.iter().copied() {
            let edit = &edits[index];
            if !edit.effective {
                continue;
            }
            let route = edit.route.as_ref().expect("route assigned");
            match &edit.update.change {
                Change::Update {
                    log:
                        LogChange {
                            mode: RefLog::AndReference,
                            ..
                        },
                    ..
                } => {
                    names.insert(route.stored.clone());
                }
                Change::Delete {
                    log: RefLog::AndReference,
                    ..
                } => {
                    names.remove(&route.stored);
                }
                _ => {}
            }
        }
        let ordered: Vec<_> = names.into_iter().collect();
        for pair in ordered.windows(2) {
            let first = pair[0].as_slice();
            let second = pair[1].as_slice();
            if second.starts_with(first) && second.get(first.len()) == Some(&b'/') {
                return Err(Error::NameConflict {
                    first: pair[0].clone(),
                    second: pair[1].clone(),
                });
            }
        }
    }
    Ok(())
}

fn build_table(
    store: &Store,
    objects: Option<&dyn gix_object::Find>,
    stack: &mut PreparedStack,
    edits: &[Edit],
    committer: Option<gix_actor::SignatureRef<'_>>,
) -> Result<Table, Error> {
    let update_index = stack.addition.next_update_index();
    let mut table = Table::default();
    for index in stack.edit_indices.iter().copied() {
        let edit = &edits[index];
        if !edit.effective {
            continue;
        }
        let route = edit.route.as_ref().expect("route assigned");
        match &edit.update.change {
            Change::Delete { log, .. } => {
                append_log_tombstones(&mut stack.addition, &route.stored, &mut table.logs)?;
                if *log == RefLog::AndReference {
                    table.refs.push(WriteRefRecord {
                        name: route.stored.clone(),
                        update_index,
                        value: WriteRefValue::Deletion,
                    });
                }
            }
            Change::Update { log, expected, new } => {
                if should_write_log(store, &mut stack.addition, route, log)? {
                    if let Some((old, new)) = reflog_oids(edit, expected, new) {
                        let committer = committer.ok_or(Error::MissingCommitter)?;
                        let committer = committer.trim();
                        let committer_time = committer.time().map_err(Error::InvalidCommitterTime)?;
                        table.logs.push(WriteLogRecord {
                            name: route.stored.clone(),
                            update_index,
                            value: WriteLogValue::Update {
                                old,
                                new,
                                name: committer.name.to_owned(),
                                email: committer.email.to_owned(),
                                time: committer_time
                                    .seconds
                                    .try_into()
                                    .map_err(|_| Error::NegativeTimestamp)?,
                                tz_offset: timezone_from_seconds(committer_time.offset)?,
                                message: log_message(&log.message, store.options.block_size)?,
                            },
                        });
                    }
                }
                if log.mode == RefLog::AndReference {
                    let value = match new {
                        Target::Symbolic(target) => WriteRefValue::Symbolic(target.as_bstr().to_owned()),
                        Target::Object(object) => match objects.and_then(|objects| peel(objects, object)) {
                            Some(peeled) => WriteRefValue::Peeled {
                                object: *object,
                                peeled,
                            },
                            None => WriteRefValue::Object(*object),
                        },
                    };
                    table.refs.push(WriteRefRecord {
                        name: route.stored.clone(),
                        update_index,
                        value,
                    });
                }
            }
        }
    }
    table.refs.sort_by(|a, b| a.name.cmp(&b.name));
    table
        .logs
        .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| b.update_index.cmp(&a.update_index)));
    Ok(table)
}

fn append_log_tombstones(
    addition: &mut gix_reftable::Addition,
    name: &[u8],
    out: &mut Vec<WriteLogRecord>,
) -> Result<(), Error> {
    let mut iter = addition.logs()?;
    iter.seek(name)?;
    while let Some(record) = iter.next_record()? {
        if record.name.as_bytes() != name {
            break;
        }
        out.push(WriteLogRecord {
            name: record.name.to_owned(),
            update_index: record.update_index,
            value: WriteLogValue::Deletion,
        });
    }
    Ok(())
}

fn should_write_log(
    store: &Store,
    addition: &mut gix_reftable::Addition,
    route: &RoutedName,
    log: &LogChange,
) -> Result<bool, Error> {
    if store.write_reflog == crate::store::WriteReflog::Disable {
        return Ok(false);
    }
    if store.write_reflog == crate::store::WriteReflog::Always || log.force_create_reflog {
        return Ok(true);
    }
    if store.write_reflog == crate::store::WriteReflog::Normal
        && (route.logical == "HEAD"
            || route.logical.starts_with_str("refs/heads/")
            || route.logical.starts_with_str("refs/remotes/")
            || route.logical.starts_with_str("refs/notes/"))
    {
        return Ok(true);
    }
    let mut iter = addition.logs()?;
    iter.seek(&route.stored)?;
    Ok(iter.next_record()?.is_some_and(|record| record.name == route.stored))
}

fn reflog_oids(
    edit: &Edit,
    expected: &PreviousValue,
    new: &Target,
) -> Option<(gix_hash::ObjectId, gix_hash::ObjectId)> {
    match new {
        Target::Object(new) => {
            let old = edit
                .leaf_referent_previous_oid
                .or(match expected {
                    PreviousValue::MustExistAndMatch(Target::Object(old))
                    | PreviousValue::ExistingMustMatch(Target::Object(old)) => Some(*old),
                    _ => None,
                })
                .unwrap_or_else(|| new.kind().null());
            (old != *new).then_some((old, *new))
        }
        Target::Symbolic(_) => match expected {
            PreviousValue::ExistingMustMatch(Target::Object(new)) => Some((new.kind().null(), *new)),
            _ => None,
        },
    }
}

fn timezone_from_seconds(offset: i32) -> Result<i16, Error> {
    let sign = if offset < 0 { -1 } else { 1 };
    let absolute = offset.abs();
    if absolute % 60 != 0 {
        return Err(Error::InvalidTimezone(offset));
    }
    let value = sign * ((absolute / 3600) * 100 + (absolute % 3600) / 60);
    value.try_into().map_err(|_| Error::InvalidTimezone(offset))
}

fn log_message(message: &[u8], configured_block_size: u32) -> Result<BString, Error> {
    if message.contains(&b'\0') || message.contains(&b'\n') {
        return Err(Error::InvalidLogMessage);
    }
    let block_size = if configured_block_size == 0 {
        4096
    } else {
        configured_block_size
    };
    let max_len = match usize::try_from(block_size / 2) {
        Ok(value) => value,
        Err(_) => usize::MAX,
    };
    Ok(BString::from(&message[..message.len().min(max_len)]))
}

fn peel(objects: &dyn gix_object::Find, id: &gix_hash::ObjectId) -> Option<gix_hash::ObjectId> {
    let mut current = *id;
    let mut buffer = Vec::new();
    let mut peeled = None;
    for _ in 0..32 {
        let object = objects.try_find(&current, &mut buffer).ok().flatten()?;
        if object.kind != gix_object::Kind::Tag {
            return peeled;
        }
        current = gix_object::TagRefIter::from_bytes(object.data, object.object_hash)
            .target_id()
            .ok()?;
        peeled = Some(current);
    }
    None
}
