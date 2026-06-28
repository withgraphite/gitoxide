use std::{
    ffi::{CStr, CString, c_char, c_int, c_long, c_void},
    fs::File,
    io,
    path::{Path, PathBuf},
    ptr,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{reftable_error_REFTABLE_API_ERROR, reftable_error_REFTABLE_IO_ERROR, reftable_error_REFTABLE_LOCK_ERROR};

#[derive(Clone, Copy)]
enum Failure {
    Io,
    Lock,
    Api,
}

impl Failure {
    const fn code(self) -> c_int {
        match self {
            Failure::Io => reftable_error_REFTABLE_IO_ERROR,
            Failure::Lock => reftable_error_REFTABLE_LOCK_ERROR,
            Failure::Api => reftable_error_REFTABLE_API_ERROR,
        }
    }
}

impl From<io::Error> for Failure {
    fn from(_: io::Error) -> Self {
        Failure::Io
    }
}

fn guard(f: impl FnOnce() -> Result<(), Failure>) -> c_int {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(())) => 0,
        Ok(Err(err)) => err.code(),
        Err(_) => Failure::Io.code(),
    }
}

fn path_from_c(path: *const c_char) -> Result<PathBuf, Failure> {
    if path.is_null() {
        return Err(Failure::Api);
    }
    // SAFETY: the C contract requires a NUL-terminated path.
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    gix_path::try_from_byte_slice(bytes)
        .map(Path::to_owned)
        .map_err(|_| Failure::Io)
}

fn path_to_c(path: &Path) -> Result<CString, Failure> {
    let bytes = gix_path::try_into_bstr(path).map_err(|_| Failure::Io)?;
    CString::new(bytes.as_ref().to_vec()).map_err(|_| Failure::Api)
}

#[cfg(unix)]
fn descriptor(file: &File) -> io::Result<c_int> {
    use std::os::fd::AsRawFd;
    Ok(file.as_raw_fd())
}

#[cfg(windows)]
fn descriptor(file: &File) -> io::Result<c_int> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, HANDLE},
        System::Threading::{DuplicateHandle, GetCurrentProcess},
    };

    unsafe extern "C" {
        fn _open_osfhandle(osfhandle: isize, flags: c_int) -> c_int;
    }

    // SAFETY: process pseudo-handles are always valid.
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: the source handle is live and `duplicate` points to writable storage.
    let ok = unsafe {
        DuplicateHandle(
            process,
            file.as_raw_handle() as HANDLE,
            process,
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the duplicate is transferred to the CRT descriptor.
    let fd = unsafe { _open_osfhandle(duplicate as isize, 0) };
    if fd < 0 {
        // SAFETY: `_open_osfhandle` did not consume a rejected handle.
        unsafe {
            CloseHandle(duplicate);
        }
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

#[cfg(unix)]
fn close_extra_descriptor(_fd: c_int) {}

#[cfg(windows)]
fn close_extra_descriptor(fd: c_int) {
    unsafe extern "C" {
        fn _close(fd: c_int) -> c_int;
    }
    // SAFETY: Windows descriptors produced by `descriptor()` are owned duplicates.
    unsafe {
        _close(fd);
    }
}

#[repr(C)]
struct ReftableTmpfile {
    path: *const c_char,
    fd: c_int,
    private: *mut c_void,
}

#[repr(C)]
struct ReftableLock {
    path: *const c_char,
    fd: c_int,
    private: *mut c_void,
}

#[repr(C)]
struct ReftableMmap {
    data: *mut c_void,
    size: usize,
    private: *mut c_void,
}

enum TempHandle {
    Open(gix_tempfile::Handle<gix_tempfile::handle::Writable>),
    Closed(gix_tempfile::Handle<gix_tempfile::handle::Closed>),
}

struct TempState {
    path: CString,
    handle: Option<TempHandle>,
    fd: c_int,
}

enum LockHandle {
    Open(gix_lock::File),
    Closed(gix_lock::Marker),
}

struct LockState {
    path: CString,
    handle: Option<LockHandle>,
    fd: c_int,
}

unsafe fn reset_tmpfile(file: *mut ReftableTmpfile) {
    // SAFETY: the pointer is supplied as writable storage by C.
    unsafe {
        *file = ReftableTmpfile {
            path: ptr::null(),
            fd: -1,
            private: ptr::null_mut(),
        };
    }
}

unsafe fn reset_lock(lock: *mut ReftableLock) {
    // SAFETY: the pointer is supplied as writable storage by C.
    unsafe {
        *lock = ReftableLock {
            path: ptr::null(),
            fd: -1,
            private: ptr::null_mut(),
        };
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tmpfile_from_pattern(out: *mut ReftableTmpfile, pattern: *const c_char) -> c_int {
    guard(|| {
        if out.is_null() {
            return Err(Failure::Api);
        }
        let pattern = path_from_c(pattern)?;
        if pattern.parent().is_none() {
            return Err(Failure::Api);
        }
        let mut generated: Vec<u8> = gix_path::try_into_bstr(pattern.as_path())
            .map_err(|_| Failure::Io)?
            .into_owned()
            .into();
        if !generated.ends_with(b"XXXXXX") {
            return Err(Failure::Api);
        }
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let random_start = generated.len() - 6;
        let mut rng = seeded_rng().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for byte in &mut generated[random_start..] {
            *byte = ALPHABET[rng.usize(..ALPHABET.len())];
        }
        drop(rng);
        let generated = gix_path::try_from_bstring(bstr::BString::from(generated)).map_err(|_| Failure::Io)?;
        #[cfg(unix)]
        let mut handle = {
            use std::os::unix::fs::PermissionsExt;
            gix_tempfile::writable_at_with_permissions(
                generated,
                gix_tempfile::ContainingDirectory::Exists,
                gix_tempfile::AutoRemove::Tempfile,
                std::fs::Permissions::from_mode(0o666),
            )
        }
        .map_err(Failure::from)?;
        #[cfg(not(unix))]
        let mut handle = gix_tempfile::writable_at(
            generated,
            gix_tempfile::ContainingDirectory::Exists,
            gix_tempfile::AutoRemove::Tempfile,
        )
        .map_err(Failure::from)?;
        let (path, fd) = handle
            .with_mut(|file| descriptor(file.as_file()).map(|fd| (file.path().to_owned(), fd)))
            .map_err(Failure::from)?
            .map_err(Failure::from)?;
        let state = Box::new(TempState {
            path: path_to_c(&path)?,
            handle: Some(TempHandle::Open(handle)),
            fd,
        });
        let state = Box::into_raw(state);
        // SAFETY: `out` was checked and the boxed state remains live.
        unsafe {
            (*out).path = (*state).path.as_ptr();
            (*out).fd = (*state).fd;
            (*out).private = state.cast();
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tmpfile_close(file: *mut ReftableTmpfile) -> c_int {
    guard(|| {
        if file.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: `file` was checked above.
        let state = unsafe { &mut *((*file).private.cast::<TempState>()) };
        let handle = state.handle.take().ok_or(Failure::Api)?;
        match handle {
            TempHandle::Open(handle) => {
                let fd = std::mem::replace(&mut state.fd, -1);
                close_extra_descriptor(fd);
                // SAFETY: `file` is writable C-owned storage.
                unsafe {
                    (*file).fd = -1;
                }
                state.handle = Some(TempHandle::Closed(handle.close().map_err(Failure::from)?));
            }
            TempHandle::Closed(handle) => state.handle = Some(TempHandle::Closed(handle)),
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tmpfile_delete(file: *mut ReftableTmpfile) -> c_int {
    guard(|| {
        if file.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: `file` was checked above.
        let private = unsafe { (*file).private };
        if private.is_null() {
            return Ok(());
        }
        // SAFETY: the state was allocated with `Box::into_raw`.
        let mut state = unsafe { Box::from_raw(private.cast::<TempState>()) };
        // SAFETY: ownership has moved back to Rust.
        unsafe { reset_tmpfile(file) };
        if state.fd >= 0 {
            close_extra_descriptor(state.fd);
            state.fd = -1;
        }
        drop(state);
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn tmpfile_rename(file: *mut ReftableTmpfile, destination: *const c_char) -> c_int {
    guard(|| {
        if file.is_null() {
            return Err(Failure::Api);
        }
        let destination = path_from_c(destination)?;
        // SAFETY: the C contract requires an active state.
        let mut state = unsafe { Box::from_raw((*file).private.cast::<TempState>()) };
        // SAFETY: ownership has moved back to Rust.
        unsafe { reset_tmpfile(file) };
        if state.fd >= 0 {
            close_extra_descriptor(state.fd);
            state.fd = -1;
        }
        match state.handle.take().ok_or(Failure::Api)? {
            TempHandle::Open(handle) => handle.persist(destination).map(|_| ()).map_err(|_| Failure::Io),
            TempHandle::Closed(handle) => handle.persist(destination).map_err(|_| Failure::Io),
        }
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn flock_acquire(out: *mut ReftableLock, target: *const c_char, timeout_ms: c_long) -> c_int {
    guard(|| {
        if out.is_null() {
            return Err(Failure::Api);
        }
        let target = path_from_c(target)?;
        let mut handle = if timeout_ms < 0 {
            let mut backoff = gix_lock::backoff::Quadratic::default_with_random();
            loop {
                match gix_lock::File::acquire_to_update_resource(&target, gix_lock::acquire::Fail::Immediately, None) {
                    Ok(handle) => break handle,
                    Err(gix_lock::acquire::Error::PermanentlyLocked { .. }) => {
                        if let Some(wait) = backoff.next() {
                            std::thread::sleep(wait);
                        }
                    }
                    Err(gix_lock::acquire::Error::Io(_)) => return Err(Failure::Io),
                }
            }
        } else {
            let fail = if timeout_ms == 0 {
                gix_lock::acquire::Fail::Immediately
            } else {
                gix_lock::acquire::Fail::AfterDurationWithBackoff(Duration::from_millis(timeout_ms as u64))
            };
            gix_lock::File::acquire_to_update_resource(&target, fail, None).map_err(|err| match err {
                gix_lock::acquire::Error::PermanentlyLocked { .. } => Failure::Lock,
                gix_lock::acquire::Error::Io(_) => Failure::Io,
            })?
        };
        let fd = handle.with_mut(|file| descriptor(file)).map_err(Failure::from)?;
        let state = Box::new(LockState {
            path: path_to_c(handle.lock_path())?,
            handle: Some(LockHandle::Open(handle)),
            fd,
        });
        let state = Box::into_raw(state);
        // SAFETY: `out` was checked and the boxed state remains live.
        unsafe {
            (*out).path = (*state).path.as_ptr();
            (*out).fd = (*state).fd;
            (*out).private = state.cast();
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn flock_close(lock: *mut ReftableLock) -> c_int {
    guard(|| {
        if lock.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: the C contract requires an active state.
        let state = unsafe { &mut *((*lock).private.cast::<LockState>()) };
        match state.handle.take().ok_or(Failure::Api)? {
            LockHandle::Open(handle) => {
                let fd = std::mem::replace(&mut state.fd, -1);
                close_extra_descriptor(fd);
                // SAFETY: `lock` is writable C-owned storage.
                unsafe {
                    (*lock).fd = -1;
                }
                state.handle = Some(LockHandle::Closed(handle.close().map_err(Failure::from)?));
            }
            LockHandle::Closed(handle) => state.handle = Some(LockHandle::Closed(handle)),
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn flock_release(lock: *mut ReftableLock) -> c_int {
    guard(|| {
        if lock.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: `lock` was checked above.
        let private = unsafe { (*lock).private };
        if private.is_null() {
            return Ok(());
        }
        // SAFETY: the state was allocated with `Box::into_raw`.
        let mut state = unsafe { Box::from_raw(private.cast::<LockState>()) };
        // SAFETY: ownership has moved back to Rust.
        unsafe { reset_lock(lock) };
        if state.fd >= 0 {
            close_extra_descriptor(state.fd);
            state.fd = -1;
        }
        drop(state);
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn flock_commit(lock: *mut ReftableLock) -> c_int {
    guard(|| {
        if lock.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: the C contract requires an active state.
        let mut state = unsafe { Box::from_raw((*lock).private.cast::<LockState>()) };
        // SAFETY: ownership has moved back to Rust.
        unsafe { reset_lock(lock) };
        if state.fd >= 0 {
            close_extra_descriptor(state.fd);
            state.fd = -1;
        }
        match state.handle.take().ok_or(Failure::Api)? {
            LockHandle::Open(handle) => handle.commit().map(|_| ()).map_err(|_| Failure::Io),
            LockHandle::Closed(handle) => handle.commit().map(|_| ()).map_err(|_| Failure::Io),
        }
    })
}

fn with_borrowed_file<T>(fd: c_int, f: impl FnOnce(&File) -> io::Result<T>) -> io::Result<T> {
    #[cfg(unix)]
    {
        use std::{mem::ManuallyDrop, os::fd::FromRawFd};
        // SAFETY: the descriptor remains owned by the C platform structure.
        let file = ManuallyDrop::new(unsafe { File::from_raw_fd(fd) });
        f(&file)
    }
    #[cfg(windows)]
    {
        use std::{mem::ManuallyDrop, os::windows::io::FromRawHandle};
        unsafe extern "C" {
            fn _get_osfhandle(fd: c_int) -> isize;
        }
        // SAFETY: `fd` is a live CRT descriptor.
        let handle = unsafe { _get_osfhandle(fd) };
        if handle == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the handle remains owned by the CRT descriptor.
        let file = ManuallyDrop::new(unsafe { File::from_raw_handle(handle as *mut c_void) });
        f(&file)
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn reftable_fsync(fd: c_int) -> c_int {
    guard(|| with_borrowed_file(fd, File::sync_all).map_err(Failure::from))
}

fn seeded_rng() -> &'static Mutex<fastrand::Rng> {
    static RNG: LazyLock<Mutex<fastrand::Rng>> = LazyLock::new(|| {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64);
        let process = u64::from(std::process::id());
        let address = std::ptr::from_ref(&RNG) as usize as u64;
        Mutex::new(fastrand::Rng::with_seed(time ^ process.rotate_left(17) ^ address))
    });
    &RNG
}

#[unsafe(no_mangle)]
unsafe extern "C" fn reftable_rand() -> u32 {
    seeded_rng()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .u32(..)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn reftable_time_ms() -> u64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn reftable_mmap(out: *mut ReftableMmap, fd: c_int, len: usize) -> c_int {
    guard(|| {
        if out.is_null() {
            return Err(Failure::Api);
        }
        let map = with_borrowed_file(fd, |file| {
            // SAFETY: C keeps the file alive and requests a read-only mapping.
            unsafe { memmap2::MmapOptions::new().len(len).map(file) }
        })
        .map_err(Failure::from)?;
        let map = Box::new(map);
        let data = map.as_ptr() as *mut c_void;
        let map = Box::into_raw(map);
        // SAFETY: `out` was checked and the map remains boxed.
        unsafe {
            (*out).data = data;
            (*out).size = len;
            (*out).private = map.cast();
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn reftable_munmap(map: *mut ReftableMmap) -> c_int {
    guard(|| {
        if map.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: `map` was checked above.
        let private = unsafe { (*map).private };
        if private.is_null() {
            return Err(Failure::Api);
        }
        // SAFETY: ownership moves back to Rust.
        unsafe {
            *map = ReftableMmap {
                data: ptr::null_mut(),
                size: 0,
                private: ptr::null_mut(),
            };
            drop(Box::from_raw(private.cast::<memmap2::Mmap>()));
        }
        Ok(())
    })
}
