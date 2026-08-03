//! Safe access to Git's vendored C reftable implementation.

#![deny(missing_docs, rust_2018_idioms)]

use std::{
    ffi::{CStr, CString, c_int, c_void},
    marker::PhantomData,
    path::Path,
    ptr::{self, NonNull},
};

use bstr::{BStr, ByteSlice};
mod raw;

fn hash_to_raw(hash: gix_hash::Kind) -> raw::reftable_hash {
    match hash {
        #[cfg(feature = "sha1")]
        gix_hash::Kind::Sha1 => raw::reftable_hash_REFTABLE_HASH_SHA1,
        #[cfg(feature = "sha256")]
        gix_hash::Kind::Sha256 => raw::reftable_hash_REFTABLE_HASH_SHA256,
        #[allow(unreachable_patterns)]
        _ => panic!("hash kind {hash:?} is not enabled in gix-reftable-sys"),
    }
}

fn hash_from_raw(value: raw::reftable_hash) -> gix_hash::Kind {
    match value {
        #[cfg(feature = "sha1")]
        raw::reftable_hash_REFTABLE_HASH_SHA1 => gix_hash::Kind::Sha1,
        #[cfg(feature = "sha256")]
        raw::reftable_hash_REFTABLE_HASH_SHA256 => gix_hash::Kind::Sha256,
        _ => panic!("C library returned unknown hash identifier {value}"),
    }
}

const fn default_hash() -> gix_hash::Kind {
    #[cfg(feature = "sha1")]
    {
        gix_hash::Kind::Sha1
    }
    #[cfg(all(not(feature = "sha1"), feature = "sha256"))]
    {
        gix_hash::Kind::Sha256
    }
}

/// Options controlling stack reads, writes, locking, and compaction.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Hash algorithm used by the repository.
    pub hash: gix_hash::Kind,
    /// Reftable block size, or zero for the C library default.
    pub block_size: u32,
    /// Key restart interval, or zero for the C library default.
    pub restart_interval: u16,
    /// Skip generation of the object-to-reference index.
    pub skip_index_objects: bool,
    /// Disable automatic geometric compaction.
    pub disable_auto_compact: bool,
    /// Geometric compaction factor, or zero for the C library default.
    pub auto_compaction_factor: u8,
    /// Permissions for newly created files, or zero to honor the process umask.
    pub default_permissions: u32,
    /// Milliseconds to wait for locks, where `-1` waits indefinitely.
    pub lock_timeout_ms: i64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            hash: default_hash(),
            block_size: 0,
            restart_interval: 0,
            skip_index_objects: false,
            disable_auto_compact: false,
            auto_compaction_factor: 0,
            default_permissions: 0,
            lock_timeout_ms: 100,
        }
    }
}

/// A stable classification of C reftable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// An unexpected filesystem failure.
    Io,
    /// Malformed or inconsistent reftable data.
    Format,
    /// A requested file does not exist.
    NotExist,
    /// A lock could not be acquired.
    Lock,
    /// The C API was used incorrectly.
    Api,
    /// Compression or decompression failed.
    Zlib,
    /// A write produced no blocks.
    EmptyTable,
    /// A reference name was rejected.
    Refname,
    /// A record was too large.
    EntryTooBig,
    /// A write raced with a newer stack.
    Outdated,
    /// Allocation failed in the C library.
    OutOfMemory,
}

impl ErrorCode {
    fn from_raw(code: i32) -> Self {
        match code {
            raw::IO_ERROR => ErrorCode::Io,
            raw::FORMAT_ERROR => ErrorCode::Format,
            raw::NOT_EXIST_ERROR => ErrorCode::NotExist,
            raw::LOCK_ERROR => ErrorCode::Lock,
            raw::API_ERROR => ErrorCode::Api,
            raw::ZLIB_ERROR => ErrorCode::Zlib,
            raw::EMPTY_TABLE_ERROR => ErrorCode::EmptyTable,
            raw::REFNAME_ERROR => ErrorCode::Refname,
            raw::ENTRY_TOO_BIG_ERROR => ErrorCode::EntryTooBig,
            raw::OUTDATED_ERROR => ErrorCode::Outdated,
            raw::OUT_OF_MEMORY_ERROR => ErrorCode::OutOfMemory,
            other => panic!("C library returned unknown error code {other}"),
        }
    }
}

/// An error returned by the reftable C library or its platform adapter.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {message}")]
pub struct Error {
    code: ErrorCode,
    operation: &'static str,
    message: String,
}

impl Error {
    /// Return the stable error classification.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    fn from_code(code: i32, operation: &'static str) -> Self {
        let code_kind = ErrorCode::from_raw(code);
        // SAFETY: known error codes return immutable process-lifetime strings.
        let message = unsafe {
            let ptr = raw::reftable_error_str(code);
            assert!(!ptr.is_null(), "C library returned a null error string for code {code}");
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        };
        Error {
            code: code_kind,
            operation,
            message,
        }
    }

    fn local(operation: &'static str, message: impl Into<String>) -> Self {
        Self::local_with_code(ErrorCode::Api, operation, message)
    }

    fn local_with_code(code: ErrorCode, operation: &'static str, message: impl Into<String>) -> Self {
        Error {
            code,
            operation,
            message: message.into(),
        }
    }
}

fn check(code: i32, operation: &'static str) -> Result<(), Error> {
    assert_ne!(
        code,
        raw::API_ERROR,
        "{operation}: reftable C API rejected an invalid call"
    );
    if code < 0 {
        Err(Error::from_code(code, operation))
    } else {
        Ok(())
    }
}

fn cstring(bytes: &[u8], operation: &'static str) -> CString {
    CString::new(bytes).unwrap_or_else(|_| panic!("{operation}: value contains a NUL byte"))
}

fn path_cstring(path: &Path) -> Result<CString, Error> {
    let bytes = gix_path::try_into_bstr(path).map_err(|err| Error {
        code: ErrorCode::Api,
        operation: "convert stack path",
        message: err.to_string(),
    })?;
    Ok(cstring(bytes.as_ref(), "convert stack path"))
}

/// A mutable reftable stack.
#[derive(Debug)]
pub struct Stack {
    raw: NonNull<raw::Stack>,
    hash: gix_hash::Kind,
}

// SAFETY: a stack has no thread affinity and safe methods require exclusive access.
unsafe impl Send for Stack {}

unsafe extern "C" fn fsck_report(_info: *mut raw::reftable_fsck_info, payload: *mut c_void) -> c_int {
    // SAFETY: `Stack::fsck()` supplies a valid mutable `usize`.
    let issues = unsafe { &mut *payload.cast::<usize>() };
    *issues += 1;
    0
}

unsafe extern "C" fn fsck_verbose(_message: *const std::ffi::c_char, _payload: *mut c_void) {}

impl Stack {
    /// Open a stack from an existing reftable directory.
    pub fn open(path: &Path, options: Options) -> Result<Self, Error> {
        if options.block_size != 0 && !(32..(1 << 24)).contains(&options.block_size) {
            return Err(Error::local(
                "open stack",
                format!("block size {} is outside the supported range", options.block_size),
            ));
        }
        if options.lock_timeout_ms < -1 {
            return Err(Error::local(
                "open stack",
                format!("lock timeout {} is invalid", options.lock_timeout_ms),
            ));
        }
        validate_stack_manifest(path)?;
        let path = path_cstring(path)?;
        let hash_id = hash_to_raw(options.hash);
        let lock_timeout_ms = std::ffi::c_long::try_from(options.lock_timeout_ms)
            .map_err(|_| Error::local("open stack", "lock timeout does not fit C long"))?;
        // SAFETY: all-zero is the documented default representation of this C options struct.
        let mut raw_options: raw::reftable_write_options = unsafe { std::mem::zeroed() };
        raw_options.block_size = options.block_size;
        raw_options.restart_interval = options.restart_interval;
        raw_options.hash_id = hash_id;
        raw_options.skip_index_objects = u8::from(options.skip_index_objects);
        raw_options.disable_auto_compact = u8::from(options.disable_auto_compact);
        raw_options.auto_compaction_factor = options.auto_compaction_factor;
        raw_options.default_permissions = options.default_permissions;
        raw_options.lock_timeout_ms = lock_timeout_ms;
        let mut raw = ptr::null_mut();
        // SAFETY: pointers remain valid for the duration of the call.
        let result = unsafe { raw::reftable_new_stack(&mut raw, path.as_ptr(), &raw_options) };
        check(result, "open stack")?;
        let raw = NonNull::new(raw).expect("successful reftable_new_stack() returns a stack");
        let stack = Stack {
            raw,
            hash: options.hash,
        };
        assert_eq!(
            stack.hash(),
            options.hash,
            "successful stack initialization uses the requested hash"
        );
        Ok(stack)
    }

    /// Reload tables changed by another stack or process.
    pub fn reload(&mut self) -> Result<(), Error> {
        // SAFETY: `self.raw` is an owned live stack.
        check(unsafe { raw::reftable_stack_reload(self.raw.as_ptr()) }, "reload stack")
    }

    /// Return the stack's hash algorithm.
    pub fn hash(&self) -> gix_hash::Kind {
        // SAFETY: `self.raw` is an owned live stack.
        let value = unsafe { raw::reftable_stack_hash_id(self.raw.as_ptr()) };
        hash_from_raw(value)
    }

    /// Return the update index for the next table.
    pub fn next_update_index(&mut self) -> u64 {
        // SAFETY: `self.raw` is an owned live stack.
        unsafe { raw::reftable_stack_next_update_index(self.raw.as_ptr()) }
    }

    /// Start iterating references in key order.
    pub fn references(&mut self) -> Result<RefIter<'_>, Error> {
        self.init_ref_iter()
    }

    /// Like [`references()`](Stack::references), but consumes the stack so the returned iterator
    /// can be stored and moved freely instead of borrowing the stack.
    pub fn into_references(mut self) -> Result<OwnedRefIter, Error> {
        let iter = self.init_ref_iter()?;
        Ok(OwnedRefIter { iter, _stack: self })
    }

    fn init_ref_iter<'a>(&mut self) -> Result<RefIter<'a>, Error> {
        // SAFETY: a zeroed iterator is the C API's documented uninitialized state.
        let mut raw: Box<raw::RefIterator> = Box::new(unsafe { std::mem::zeroed() });
        // SAFETY: the C iterator borrows the heap-allocated C stack, which the caller keeps alive
        // for the (freely chosen) iterator lifetime.
        let result = unsafe { raw::reftable_stack_init_ref_iterator(self.raw.as_ptr(), raw.as_mut()) };
        check(result, "create reference iterator")?;
        Ok(RefIter {
            raw,
            // SAFETY: the release function accepts a zero-initialized record.
            record: unsafe { std::mem::zeroed() },
            hash: self.hash,
            _stack: PhantomData,
        })
    }

    /// Start iterating reflog records in key order.
    pub fn logs(&mut self) -> Result<LogIter<'_>, Error> {
        // SAFETY: a zeroed iterator is the C API's documented uninitialized state.
        let mut raw: Box<raw::LogIterator> = Box::new(unsafe { std::mem::zeroed() });
        // SAFETY: the stack is exclusively borrowed for the iterator lifetime.
        let result = unsafe { raw::reftable_stack_init_log_iterator(self.raw.as_ptr(), raw.as_mut()) };
        check(result, "create log iterator")?;
        Ok(LogIter {
            raw,
            // SAFETY: the release function accepts a zero-initialized record.
            record: unsafe { std::mem::zeroed() },
            hash: self.hash,
            _stack: PhantomData,
        })
    }

    /// Lock this stack and begin an atomic table addition.
    pub fn into_addition(self) -> Result<Addition, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: the stack remains owned by the returned addition.
        let result = unsafe { raw::reftable_stack_new_addition(&mut raw, self.raw.as_ptr(), raw::ADDITION_RELOAD) };
        check(result, "prepare stack addition")?;
        Ok(Addition {
            raw: NonNull::new(raw).expect("successful reftable_stack_new_addition() returns an addition"),
            stack: self,
        })
    }

    /// Heuristically compact an unbalanced stack.
    pub fn auto_compact(&mut self) -> Result<(), Error> {
        // SAFETY: `self.raw` is an owned live stack.
        check(
            unsafe { raw::reftable_stack_auto_compact(self.raw.as_ptr()) },
            "auto-compact stack",
        )
    }

    /// Compact all tables in the stack.
    pub fn compact_all(&mut self) -> Result<(), Error> {
        // SAFETY: `self.raw` is an owned live stack.
        check(
            unsafe { raw::reftable_stack_compact_all(self.raw.as_ptr(), ptr::null_mut()) },
            "compact stack",
        )
    }

    /// Delete stale tables no longer referenced by the manifest.
    pub fn clean(&mut self) -> Result<(), Error> {
        // SAFETY: `self.raw` is an owned live stack.
        check(unsafe { raw::reftable_stack_clean(self.raw.as_ptr()) }, "clean stack")
    }

    /// Run the checks implemented by the vendored C library and return the number of issues.
    pub fn fsck(&mut self) -> Result<usize, Error> {
        let mut issues = 0;
        // SAFETY: the stack is live and `issues` points to writable storage.
        check(
            unsafe {
                raw::reftable_fsck_check(
                    self.raw.as_ptr(),
                    Some(fsck_report),
                    Some(fsck_verbose),
                    std::ptr::from_mut(&mut issues).cast(),
                )
            },
            "check stack",
        )?;
        Ok(issues)
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner and drop runs once.
        unsafe { raw::reftable_stack_destroy(self.raw.as_ptr()) };
    }
}

/// A borrowed reference-record value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefValue<'a> {
    /// A tombstone hiding an older record.
    Deletion,
    /// A direct object ID.
    Object(&'a gix_hash::oid),
    /// An object ID and its peeled target.
    Peeled {
        /// Referenced object ID.
        object: &'a gix_hash::oid,
        /// Fully peeled object ID.
        peeled: &'a gix_hash::oid,
    },
    /// A symbolic reference target.
    Symbolic(&'a BStr),
}

/// A reference record borrowing its data from the iterator that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefRecord<'a> {
    /// Reference name.
    pub name: &'a BStr,
    /// Logical update index.
    pub update_index: u64,
    /// Stored value.
    pub value: RefValue<'a>,
}

/// A reference iterator borrowing its stack exclusively.
pub struct RefIter<'stack> {
    raw: Box<raw::RefIterator>,
    record: raw::reftable_ref_record,
    hash: gix_hash::Kind,
    _stack: PhantomData<&'stack mut Stack>,
}

impl RefIter<'_> {
    /// Seek so the next record is at or after `name`.
    pub fn seek(&mut self, name: &[u8]) -> Result<(), Error> {
        let name = cstring(name, "seek reference iterator");
        // SAFETY: iterator and C string are valid.
        check(
            unsafe { raw::reftable_iterator_seek_ref(self.raw.as_mut(), name.as_ptr()) },
            "seek reference iterator",
        )
    }

    /// Return the next record, or `None` at end of input.
    ///
    /// The record borrows this iterator and must be dropped before seeking or advancing again.
    pub fn next_record(&mut self) -> Result<Option<RefRecord<'_>>, Error> {
        // SAFETY: both objects are initialized and exclusively borrowed.
        unsafe { raw::reftable_ref_record_release(&mut self.record) };
        // SAFETY: iterator and output record are valid.
        let result = unsafe { raw::reftable_iterator_next_ref(self.raw.as_mut(), &mut self.record) };
        if result > 0 {
            return Ok(None);
        }
        check(result, "read reference record")?;
        // SAFETY: the C record owns this string until the next mutable iterator operation.
        let name = unsafe { borrow_c_string(self.record.refname, "read reference name") };
        let value = match self.record.value_type {
            raw::REF_DELETION => RefValue::Deletion,
            raw::REF_VAL1 => {
                // SAFETY: the value tag guarantees a direct object ID.
                let value = unsafe { raw::reftable_ref_record_val1(&self.record) };
                // SAFETY: the inline object ID remains valid with the current C record.
                RefValue::Object(unsafe { borrow_object_id(value, self.hash, "read object id") })
            }
            raw::REF_VAL2 => RefValue::Peeled {
                // SAFETY: the value tag guarantees both object-ID accessors are valid.
                object: unsafe {
                    borrow_object_id(raw::reftable_ref_record_val1(&self.record), self.hash, "read object id")
                },
                peeled: unsafe {
                    borrow_object_id(
                        raw::reftable_ref_record_val2(&self.record),
                        self.hash,
                        "read peeled object id",
                    )
                },
            },
            raw::REF_SYMREF => {
                // SAFETY: the value tag selects the `symref` union member.
                RefValue::Symbolic(unsafe { borrow_c_string(self.record.value.symref, "read symbolic target") })
            }
            other => panic!("C iterator returned unknown reference value type {other}"),
        };
        Ok(Some(RefRecord {
            name,
            update_index: self.record.update_index,
            value,
        }))
    }
}

impl Drop for RefIter<'_> {
    fn drop(&mut self) {
        // SAFETY: record and iterator are initialized, uniquely owned, and released once.
        unsafe {
            raw::reftable_ref_record_release(&mut self.record);
            raw::reftable_iterator_destroy(self.raw.as_mut());
        }
    }
}

/// A reference iterator that owns the stack it reads from, created with
/// [`Stack::into_references()`].
pub struct OwnedRefIter {
    // The C iterator points into stack-owned data; `iter` is declared first so it is dropped
    // before the stack it borrows from.
    iter: RefIter<'static>,
    _stack: Stack,
}

// SAFETY: like `Stack`, the C iterator has no thread affinity and safe methods require `&mut`.
unsafe impl Send for OwnedRefIter {}

impl OwnedRefIter {
    /// Seek so the next record is at or after `name`.
    pub fn seek(&mut self, name: &[u8]) -> Result<(), Error> {
        self.iter.seek(name)
    }

    /// Return the next record, or `None` at end of input.
    ///
    /// The record borrows this iterator and must be dropped before seeking or advancing again.
    pub fn next_record(&mut self) -> Result<Option<RefRecord<'_>>, Error> {
        self.iter.next_record()
    }
}

/// A borrowed reflog-record value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogValue<'a> {
    /// A tombstone hiding a specific older log entry.
    Deletion,
    /// A reflog update.
    Update {
        /// Previous object ID.
        old: &'a gix_hash::oid,
        /// New object ID.
        new: &'a gix_hash::oid,
        /// Committer name.
        name: &'a BStr,
        /// Committer email.
        email: &'a BStr,
        /// Seconds since the Unix epoch.
        time: u64,
        /// Signed timezone in Git's `HHMM` representation.
        tz_offset: i16,
        /// Reflog message as stored.
        message: &'a BStr,
    },
}

/// A reflog record borrowing its data from the iterator that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRecord<'a> {
    /// Reference name.
    pub name: &'a BStr,
    /// Logical update index.
    pub update_index: u64,
    /// Stored value.
    pub value: LogValue<'a>,
}

/// A reflog iterator borrowing its stack exclusively.
pub struct LogIter<'stack> {
    raw: Box<raw::LogIterator>,
    record: raw::reftable_log_record,
    hash: gix_hash::Kind,
    _stack: PhantomData<&'stack mut Stack>,
}

impl LogIter<'_> {
    /// Seek to the newest log record for `name`.
    pub fn seek(&mut self, name: &[u8]) -> Result<(), Error> {
        let name = cstring(name, "seek log iterator");
        // SAFETY: iterator and C string are valid.
        check(
            unsafe { raw::reftable_iterator_seek_log(self.raw.as_mut(), name.as_ptr()) },
            "seek log iterator",
        )
    }

    /// Return the next record, or `None` at end of input.
    ///
    /// The record borrows this iterator and must be dropped before seeking or advancing again.
    pub fn next_record(&mut self) -> Result<Option<LogRecord<'_>>, Error> {
        // SAFETY: both objects are initialized and exclusively borrowed.
        unsafe { raw::reftable_log_record_release(&mut self.record) };
        // SAFETY: iterator and output record are valid.
        let result = unsafe { raw::reftable_iterator_next_log(self.raw.as_mut(), &mut self.record) };
        if result > 0 {
            return Ok(None);
        }
        check(result, "read log record")?;
        // SAFETY: the C record owns this string until the next mutable iterator operation.
        let name = unsafe { borrow_c_string(self.record.refname, "read log name") };
        let value = match self.record.value_type {
            raw::LOG_DELETION => LogValue::Deletion,
            raw::LOG_UPDATE => {
                // SAFETY: the value tag selects the `update` union member.
                let update = unsafe { &self.record.value.update };
                LogValue::Update {
                    // SAFETY: hashes are inline in the current C record.
                    old: unsafe { borrow_object_id(update.old_hash.as_ptr(), self.hash, "read old object id") },
                    // SAFETY: hashes are inline in the current C record.
                    new: unsafe { borrow_object_id(update.new_hash.as_ptr(), self.hash, "read new object id") },
                    // SAFETY: strings remain owned by the current C record.
                    name: unsafe { borrow_c_string(update.name, "read committer name") },
                    // SAFETY: strings remain owned by the current C record.
                    email: unsafe { borrow_c_string(update.email, "read committer email") },
                    time: update.time,
                    tz_offset: update.tz_offset,
                    // SAFETY: strings remain owned by the current C record.
                    message: unsafe { borrow_c_string(update.message, "read log message") },
                }
            }
            other => panic!("C iterator returned unknown log value type {other}"),
        };
        Ok(Some(LogRecord {
            name,
            update_index: self.record.update_index,
            value,
        }))
    }
}

impl Drop for LogIter<'_> {
    fn drop(&mut self) {
        // SAFETY: record and iterator are initialized, uniquely owned, and released once.
        unsafe {
            raw::reftable_log_record_release(&mut self.record);
            raw::reftable_iterator_destroy(self.raw.as_mut());
        }
    }
}

/// A locked transaction that can add one table to a stack.
pub struct Addition {
    raw: NonNull<raw::Addition>,
    stack: Stack,
}

impl Addition {
    /// Return the update index for the table being prepared.
    pub fn next_update_index(&mut self) -> u64 {
        self.stack.next_update_index()
    }

    /// Iterate references from the locked stack snapshot.
    pub fn references(&mut self) -> Result<RefIter<'_>, Error> {
        self.stack.references()
    }

    /// Iterate reflogs from the locked stack snapshot.
    pub fn logs(&mut self) -> Result<LogIter<'_>, Error> {
        self.stack.logs()
    }

    /// Write one table through the C writer callback.
    pub fn add_table<F>(&mut self, write: F) -> Result<(), Error>
    where
        F: FnOnce(&mut Writer<'_>) -> Result<(), Error>,
    {
        struct State<F> {
            write: Option<F>,
            error: Option<Error>,
            hash: gix_hash::Kind,
        }

        unsafe extern "C" fn trampoline<F>(writer: *mut raw::Writer, payload: *mut c_void) -> c_int
        where
            F: FnOnce(&mut Writer<'_>) -> Result<(), Error>,
        {
            // SAFETY: payload points to a live state for the duration of the C call.
            let state = unsafe { &mut *payload.cast::<State<F>>() };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let write = state
                    .write
                    .take()
                    .expect("C library invokes the table writer exactly once");
                let raw = NonNull::new(writer).expect("C library supplied a null writer");
                write(&mut Writer {
                    raw,
                    hash: state.hash,
                    _lifetime: PhantomData,
                })
            }));
            match outcome {
                Ok(Ok(())) => 0,
                Ok(Err(err)) => {
                    let code = match err.code {
                        ErrorCode::Io => raw::IO_ERROR,
                        ErrorCode::Format => raw::FORMAT_ERROR,
                        ErrorCode::NotExist => raw::NOT_EXIST_ERROR,
                        ErrorCode::Lock => raw::LOCK_ERROR,
                        ErrorCode::Api => raw::API_ERROR,
                        ErrorCode::Zlib => raw::ZLIB_ERROR,
                        ErrorCode::EmptyTable => raw::EMPTY_TABLE_ERROR,
                        ErrorCode::Refname => raw::REFNAME_ERROR,
                        ErrorCode::EntryTooBig => raw::ENTRY_TOO_BIG_ERROR,
                        ErrorCode::Outdated => raw::OUTDATED_ERROR,
                        ErrorCode::OutOfMemory => raw::OUT_OF_MEMORY_ERROR,
                    };
                    state.error = Some(err);
                    code
                }
                Err(_) => {
                    state.error = Some(Error::local("write reftable", "writer callback panicked"));
                    raw::API_ERROR
                }
            }
        }

        let mut state = State {
            write: Some(write),
            error: None,
            hash: self.stack.hash,
        };
        // SAFETY: callback and payload remain valid until this synchronous call returns.
        let result = unsafe {
            raw::reftable_addition_add(
                self.raw.as_ptr(),
                Some(trampoline::<F>),
                std::ptr::from_mut(&mut state).cast(),
            )
        };
        if let Some(error) = state.error {
            return Err(error);
        }
        check(result, "add table")
    }

    /// Atomically publish all tables added to this stack transaction.
    pub fn commit(self) -> Result<Stack, Error> {
        // SAFETY: the addition is owned and has not been committed.
        check(
            unsafe { raw::reftable_addition_commit(self.raw.as_ptr()) },
            "commit stack addition",
        )?;
        // SAFETY: the addition no longer owns a lock after commit.
        unsafe { raw::reftable_addition_destroy(self.raw.as_ptr()) };
        let stack = unsafe { ptr::read(&self.stack) };
        std::mem::forget(self);
        Ok(stack)
    }
}

impl Drop for Addition {
    fn drop(&mut self) {
        // SAFETY: the addition is uniquely owned and must always be destroyed.
        unsafe { raw::reftable_addition_destroy(self.raw.as_ptr()) };
    }
}

/// A borrowed C table writer.
pub struct Writer<'writer> {
    raw: NonNull<raw::Writer>,
    hash: gix_hash::Kind,
    _lifetime: PhantomData<&'writer mut raw::Writer>,
}

impl Writer<'_> {
    /// Set the inclusive update-index range for records written next.
    pub fn set_limits(&mut self, min: u64, max: u64) {
        assert!(min <= max, "writer update-index limits must not be inverted");
        assert_ne!(
            max,
            u64::MAX,
            "writer update-index range must leave room for the next table"
        );
        // SAFETY: writer is live for the callback duration and limits were validated.
        let result = unsafe { raw::reftable_writer_set_limits(self.raw.as_ptr(), min, max) };
        assert_eq!(result, 0, "new C writer accepts valid update-index limits");
    }

    /// Add a reference record. Records must be supplied in key order.
    pub fn add_ref(&mut self, record: RefRecord<'_>) -> Result<(), Error> {
        let name = cstring(record.name, "write reference name");
        let mut object = [0_u8; raw::HASH_SIZE_MAX];
        let mut peeled = [0_u8; raw::HASH_SIZE_MAX];
        let mut symbolic = None;
        // SAFETY: a zeroed record represents a deletion with no owned pointers.
        let mut raw_record: raw::reftable_ref_record = unsafe { std::mem::zeroed() };
        raw_record.refname = name.as_ptr().cast_mut();
        raw_record.update_index = record.update_index;
        let value_type = match record.value {
            RefValue::Deletion => {
                raw_record.value = raw::reftable_ref_record__bindgen_ty_2 { val1: object };
                raw::REF_DELETION
            }
            RefValue::Object(value) => {
                copy_into_hash(value.as_bytes(), &mut object, self.hash);
                raw_record.value = raw::reftable_ref_record__bindgen_ty_2 { val1: object };
                raw::REF_VAL1
            }
            RefValue::Peeled {
                object: value,
                peeled: target,
            } => {
                copy_into_hash(value.as_bytes(), &mut object, self.hash);
                copy_into_hash(target.as_bytes(), &mut peeled, self.hash);
                raw_record.value = raw::reftable_ref_record__bindgen_ty_2 {
                    val2: raw::reftable_ref_record__bindgen_ty_2__bindgen_ty_1 {
                        value: object,
                        target_value: peeled,
                    },
                };
                raw::REF_VAL2
            }
            RefValue::Symbolic(target) => {
                let value = cstring(target, "write symbolic target");
                raw_record.value = raw::reftable_ref_record__bindgen_ty_2 {
                    symref: value.as_ptr().cast_mut(),
                };
                symbolic = Some(value);
                raw::REF_SYMREF
            }
        };
        raw_record.value_type = value_type;
        let _keep_symbolic_alive = symbolic;
        // SAFETY: all pointers remain valid for this synchronous encoding call.
        check(
            unsafe { raw::reftable_writer_add_ref(self.raw.as_ptr(), &mut raw_record) },
            "write reference record",
        )
    }

    /// Add a reflog record. Reference records must all be written first.
    pub fn add_log(&mut self, record: LogRecord<'_>) -> Result<(), Error> {
        let refname = cstring(record.name, "write log name");
        let mut old = [0_u8; raw::HASH_SIZE_MAX];
        let mut new = [0_u8; raw::HASH_SIZE_MAX];
        let mut committer_name = None;
        let mut committer_email = None;
        let mut message = None;
        let (value_type, time, tz_offset) = match record.value {
            LogValue::Deletion => (raw::LOG_DELETION, 0, 0),
            LogValue::Update {
                old: old_value,
                new: new_value,
                name,
                email,
                time,
                tz_offset,
                message: log_message,
            } => {
                copy_into_hash(old_value.as_bytes(), &mut old, self.hash);
                copy_into_hash(new_value.as_bytes(), &mut new, self.hash);
                committer_name = Some(cstring(name, "write committer name"));
                committer_email = Some(cstring(email, "write committer email"));
                message = Some(cstring(log_message, "write log message"));
                (raw::LOG_UPDATE, time, tz_offset)
            }
        };
        // SAFETY: a zeroed log record is a deletion with no owned pointers.
        let mut raw_record: raw::reftable_log_record = unsafe { std::mem::zeroed() };
        raw_record.refname = refname.as_ptr().cast_mut();
        raw_record.update_index = record.update_index;
        raw_record.value_type = value_type;
        if value_type == raw::LOG_UPDATE {
            raw_record.value = raw::reftable_log_record__bindgen_ty_2 {
                update: raw::reftable_log_record__bindgen_ty_2__bindgen_ty_1 {
                    new_hash: new,
                    old_hash: old,
                    name: committer_name
                        .as_ref()
                        .map_or(ptr::null_mut(), |value| value.as_ptr().cast_mut()),
                    email: committer_email
                        .as_ref()
                        .map_or(ptr::null_mut(), |value| value.as_ptr().cast_mut()),
                    time,
                    tz_offset,
                    message: message
                        .as_ref()
                        .map_or(ptr::null_mut(), |value| value.as_ptr().cast_mut()),
                    message_cap: 0,
                },
            };
        }
        // SAFETY: all pointers remain valid for this synchronous encoding call.
        check(
            unsafe { raw::reftable_writer_add_log(self.raw.as_ptr(), &mut raw_record) },
            "write log record",
        )
    }
}

unsafe fn borrow_c_string<'a>(value: *const std::ffi::c_char, operation: &'static str) -> &'a BStr {
    assert!(!value.is_null(), "{operation}: C library returned a null string");
    // SAFETY: the caller guarantees the C string remains valid for `'a`.
    unsafe { CStr::from_ptr(value) }.to_bytes().as_bstr()
}

unsafe fn borrow_object_id<'a>(value: *const u8, hash: gix_hash::Kind, operation: &'static str) -> &'a gix_hash::oid {
    assert!(!value.is_null(), "{operation}: C library returned a null object ID");
    // SAFETY: the caller guarantees the C record remains valid for `'a`.
    let bytes = unsafe { std::slice::from_raw_parts(value, hash.len_in_bytes()) };
    gix_hash::oid::from_bytes_unchecked(bytes)
}

fn copy_into_hash(value: &[u8], out: &mut [u8; raw::HASH_SIZE_MAX], hash: gix_hash::Kind) {
    assert_eq!(
        value.len(),
        hash.len_in_bytes(),
        "object ID kind must match the writer's hash kind"
    );
    out[..value.len()].copy_from_slice(value);
}

fn validate_stack_manifest(path: &Path) -> Result<(), Error> {
    use std::io::Read;

    let manifest = match std::fs::read(path.join("tables.list")) {
        Ok(manifest) => manifest,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Ok(()),
    };
    for name in manifest.split(|byte| *byte == b'\n').filter(|name| !name.is_empty()) {
        if name.contains(&b'/') || name.contains(&b'\\') || !name.ends_with(b".ref") {
            return Err(Error::local_with_code(
                ErrorCode::Format,
                "validate stack manifest",
                format!("invalid table basename {:?}", name.as_bstr()),
            ));
        }
        let table_name = gix_path::try_from_byte_slice(name).map_err(|_| {
            Error::local_with_code(
                ErrorCode::Format,
                "validate stack manifest",
                "table basename is not representable on this platform",
            )
        })?;
        let table_path = path.join(table_name);
        let Ok(metadata) = std::fs::metadata(&table_path) else {
            continue;
        };
        let mut header = [0_u8; 5];
        let Ok(mut file) = std::fs::File::open(&table_path) else {
            continue;
        };
        if file.read_exact(&mut header).is_err() {
            return Err(Error::local_with_code(
                ErrorCode::Format,
                "validate reftable",
                format!("table {:?} is truncated", name.as_bstr()),
            ));
        }
        let minimum_size = match &header {
            b"REFT\x01" => 24 + 1 + 68,
            b"REFT\x02" => 28 + 1 + 72,
            _ => {
                return Err(Error::local_with_code(
                    ErrorCode::Format,
                    "validate reftable",
                    format!("table {:?} has an invalid header", name.as_bstr()),
                ));
            }
        };
        if metadata.len() < minimum_size {
            return Err(Error::local_with_code(
                ErrorCode::Format,
                "validate reftable",
                format!("table {:?} is shorter than its header and footer", name.as_bstr()),
            ));
        }
    }
    Ok(())
}
