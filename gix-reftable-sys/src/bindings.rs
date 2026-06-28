pub const REFTABLE_HASH_SIZE_SHA1: u32 = 20;
pub const REFTABLE_HASH_SIZE_SHA256: u32 = 32;
pub const REFTABLE_HASH_SIZE_MAX: u32 = 32;
pub const REFTABLE_NR_REF_VALUETYPES: u32 = 4;
pub const REFTABLE_NR_LOG_VALUETYPES: u32 = 2;
pub const REFTABLE_BLOCK_TYPE_LOG: u8 = 103u8;
pub const REFTABLE_BLOCK_TYPE_INDEX: u8 = 105u8;
pub const REFTABLE_BLOCK_TYPE_REF: u8 = 114u8;
pub const REFTABLE_BLOCK_TYPE_OBJ: u8 = 111u8;
pub const REFTABLE_BLOCK_TYPE_ANY: u32 = 0;
pub type Byte = ::std::os::raw::c_uchar;
pub type uInt = ::std::os::raw::c_uint;
pub type uLong = ::std::os::raw::c_ulong;
pub type Bytef = Byte;
pub type voidpf = *mut ::std::os::raw::c_void;
pub type alloc_func = ::std::option::Option<unsafe extern "C" fn(opaque: voidpf, items: uInt, size: uInt) -> voidpf>;
pub type free_func = ::std::option::Option<unsafe extern "C" fn(opaque: voidpf, address: voidpf)>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct internal_state {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct z_stream_s {
    pub next_in: *mut Bytef,
    pub avail_in: uInt,
    pub total_in: uLong,
    pub next_out: *mut Bytef,
    pub avail_out: uInt,
    pub total_out: uLong,
    pub msg: *mut ::std::os::raw::c_char,
    pub state: *mut internal_state,
    pub zalloc: alloc_func,
    pub zfree: free_func,
    pub opaque: voidpf,
    pub data_type: ::std::os::raw::c_int,
    pub adler: uLong,
    pub reserved: uLong,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_buf {
    pub alloc: usize,
    pub len: usize,
    pub buf: *mut ::std::os::raw::c_char,
}
pub const reftable_hash_REFTABLE_HASH_SHA1: reftable_hash = 89;
pub const reftable_hash_REFTABLE_HASH_SHA256: reftable_hash = 247;
pub type reftable_hash = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_block_source {
    pub ops: *mut reftable_block_source_vtable,
    pub arg: *mut ::std::os::raw::c_void,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_block_data {
    pub data: *mut u8,
    pub len: usize,
    pub source: reftable_block_source,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_block_source_vtable {
    pub size: ::std::option::Option<unsafe extern "C" fn(source: *mut ::std::os::raw::c_void) -> u64>,
    pub read_data: ::std::option::Option<
        unsafe extern "C" fn(
            source: *mut ::std::os::raw::c_void,
            dest: *mut reftable_block_data,
            off: u64,
            size: u32,
        ) -> isize,
    >,
    pub release_data: ::std::option::Option<
        unsafe extern "C" fn(source: *mut ::std::os::raw::c_void, data: *mut reftable_block_data),
    >,
    pub close: ::std::option::Option<unsafe extern "C" fn(source: *mut ::std::os::raw::c_void)>,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct reftable_ref_record {
    pub refname: *mut ::std::os::raw::c_char,
    pub refname_cap: usize,
    pub update_index: u64,
    pub value_type: reftable_ref_record__bindgen_ty_1,
    pub value: reftable_ref_record__bindgen_ty_2,
}
pub const reftable_ref_record_REFTABLE_REF_DELETION: reftable_ref_record__bindgen_ty_1 = 0;
pub const reftable_ref_record_REFTABLE_REF_VAL1: reftable_ref_record__bindgen_ty_1 = 1;
pub const reftable_ref_record_REFTABLE_REF_VAL2: reftable_ref_record__bindgen_ty_1 = 2;
pub const reftable_ref_record_REFTABLE_REF_SYMREF: reftable_ref_record__bindgen_ty_1 = 3;
pub type reftable_ref_record__bindgen_ty_1 = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Copy, Clone)]
pub union reftable_ref_record__bindgen_ty_2 {
    pub val1: [::std::os::raw::c_uchar; 32usize],
    pub val2: reftable_ref_record__bindgen_ty_2__bindgen_ty_1,
    pub symref: *mut ::std::os::raw::c_char,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_ref_record__bindgen_ty_2__bindgen_ty_1 {
    pub value: [::std::os::raw::c_uchar; 32usize],
    pub target_value: [::std::os::raw::c_uchar; 32usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct reftable_log_record {
    pub refname: *mut ::std::os::raw::c_char,
    pub refname_cap: usize,
    pub update_index: u64,
    pub value_type: reftable_log_record__bindgen_ty_1,
    pub value: reftable_log_record__bindgen_ty_2,
}
pub const reftable_log_record_REFTABLE_LOG_DELETION: reftable_log_record__bindgen_ty_1 = 0;
pub const reftable_log_record_REFTABLE_LOG_UPDATE: reftable_log_record__bindgen_ty_1 = 1;
pub type reftable_log_record__bindgen_ty_1 = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Copy, Clone)]
pub union reftable_log_record__bindgen_ty_2 {
    pub update: reftable_log_record__bindgen_ty_2__bindgen_ty_1,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_log_record__bindgen_ty_2__bindgen_ty_1 {
    pub new_hash: [::std::os::raw::c_uchar; 32usize],
    pub old_hash: [::std::os::raw::c_uchar; 32usize],
    pub name: *mut ::std::os::raw::c_char,
    pub email: *mut ::std::os::raw::c_char,
    pub time: u64,
    pub tz_offset: i16,
    pub message: *mut ::std::os::raw::c_char,
    pub message_cap: usize,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_iterator_vtable {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_iterator {
    pub ops: *mut reftable_iterator_vtable,
    pub iter_arg: *mut ::std::os::raw::c_void,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_block {
    pub header_off: u32,
    pub block_data: reftable_block_data,
    pub hash_size: u32,
    pub zstream: *mut z_stream_s,
    pub uncompressed_data: *mut ::std::os::raw::c_uchar,
    pub uncompressed_cap: usize,
    pub restart_count: u16,
    pub restart_off: u32,
    pub full_block_size: u32,
    pub block_type: u8,
}
pub const reftable_error_REFTABLE_IO_ERROR: reftable_error = -2;
pub const reftable_error_REFTABLE_FORMAT_ERROR: reftable_error = -3;
pub const reftable_error_REFTABLE_NOT_EXIST_ERROR: reftable_error = -4;
pub const reftable_error_REFTABLE_LOCK_ERROR: reftable_error = -5;
pub const reftable_error_REFTABLE_API_ERROR: reftable_error = -6;
pub const reftable_error_REFTABLE_ZLIB_ERROR: reftable_error = -7;
pub const reftable_error_REFTABLE_EMPTY_TABLE_ERROR: reftable_error = -8;
pub const reftable_error_REFTABLE_REFNAME_ERROR: reftable_error = -10;
pub const reftable_error_REFTABLE_ENTRY_TOO_BIG_ERROR: reftable_error = -11;
pub const reftable_error_REFTABLE_OUTDATED_ERROR: reftable_error = -12;
pub const reftable_error_REFTABLE_OUT_OF_MEMORY_ERROR: reftable_error = -13;
pub type reftable_error = ::std::os::raw::c_int;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_write_options {
    pub unpadded: u8,
    pub block_size: u32,
    pub skip_index_objects: u8,
    pub restart_interval: u16,
    pub hash_id: reftable_hash,
    pub default_permissions: ::std::os::raw::c_uint,
    pub exact_log_message: u8,
    pub disable_auto_compact: u8,
    pub auto_compaction_factor: u8,
    pub lock_timeout_ms: ::std::os::raw::c_long,
    pub on_reload: ::std::option::Option<unsafe extern "C" fn(payload: *mut ::std::os::raw::c_void)>,
    pub on_reload_payload: *mut ::std::os::raw::c_void,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_block_stats {
    pub entries: ::std::os::raw::c_int,
    pub restarts: u32,
    pub blocks: ::std::os::raw::c_int,
    pub index_blocks: ::std::os::raw::c_int,
    pub max_index_level: ::std::os::raw::c_int,
    pub offset: u64,
    pub index_offset: u64,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_stats {
    pub blocks: ::std::os::raw::c_int,
    pub ref_stats: reftable_block_stats,
    pub obj_stats: reftable_block_stats,
    pub idx_stats: reftable_block_stats,
    pub log_stats: reftable_block_stats,
    pub object_id_len: ::std::os::raw::c_int,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_writer {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_stack {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_addition {
    _unused: [u8; 0],
}
pub const REFTABLE_STACK_NEW_ADDITION_RELOAD: _bindgen_ty_1 = 1;
pub type _bindgen_ty_1 = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_merged_table {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_log_expiry_config {
    pub time: u64,
    pub min_update_index: u64,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_compaction_stats {
    pub bytes: u64,
    pub entries_written: u64,
    pub attempts: ::std::os::raw::c_int,
    pub failures: ::std::os::raw::c_int,
}
pub const reftable_fsck_error_REFTABLE_FSCK_ERROR_TABLE_NAME: reftable_fsck_error = 0;
pub const reftable_fsck_error_REFTABLE_FSCK_MAX_VALUE: reftable_fsck_error = 1;
pub type reftable_fsck_error = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_fsck_info {
    pub error: reftable_fsck_error,
    pub msg: *const ::std::os::raw::c_char,
    pub path: *const ::std::os::raw::c_char,
}
pub type reftable_fsck_report_fn = ::std::option::Option<
    unsafe extern "C" fn(info: *mut reftable_fsck_info, cb_data: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int,
>;
pub type reftable_fsck_verbose_fn = ::std::option::Option<
    unsafe extern "C" fn(msg: *const ::std::os::raw::c_char, cb_data: *mut ::std::os::raw::c_void),
>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_table_offsets {
    pub is_present: ::std::os::raw::c_int,
    pub offset: u64,
    pub index_offset: u64,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_table {
    pub name: *mut ::std::os::raw::c_char,
    pub source: reftable_block_source,
    pub size: u64,
    pub hash_id: reftable_hash,
    pub block_size: u32,
    pub min_update_index: u64,
    pub max_update_index: u64,
    pub object_id_len: ::std::os::raw::c_int,
    pub version: ::std::os::raw::c_int,
    pub ref_offsets: reftable_table_offsets,
    pub obj_offsets: reftable_table_offsets,
    pub log_offsets: reftable_table_offsets,
    pub refcount: u64,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct reftable_table_iterator {
    pub iter_arg: *mut ::std::os::raw::c_void,
}
unsafe extern "C" {
    pub fn reftable_fsync(fd: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
    pub fn reftable_set_alloc(
        malloc: ::std::option::Option<unsafe extern "C" fn(arg1: usize) -> *mut ::std::os::raw::c_void>,
        realloc: ::std::option::Option<
            unsafe extern "C" fn(arg1: *mut ::std::os::raw::c_void, arg2: usize) -> *mut ::std::os::raw::c_void,
        >,
        free: ::std::option::Option<unsafe extern "C" fn(arg1: *mut ::std::os::raw::c_void)>,
    );
    pub fn reftable_block_source_from_file(
        block_src: *mut reftable_block_source,
        name: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_ref_record_val1(rec: *const reftable_ref_record) -> *const ::std::os::raw::c_uchar;
    pub fn reftable_ref_record_val2(rec: *const reftable_ref_record) -> *const ::std::os::raw::c_uchar;
    pub fn reftable_ref_record_is_deletion(ref_: *const reftable_ref_record) -> ::std::os::raw::c_int;
    pub fn reftable_ref_record_release(ref_: *mut reftable_ref_record);
    pub fn reftable_ref_record_equal(
        a: *const reftable_ref_record,
        b: *const reftable_ref_record,
        hash_size: u32,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_log_record_is_deletion(log: *const reftable_log_record) -> ::std::os::raw::c_int;
    pub fn reftable_log_record_release(log: *mut reftable_log_record);
    pub fn reftable_log_record_equal(
        a: *const reftable_log_record,
        b: *const reftable_log_record,
        hash_size: u32,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_seek_ref(
        it: *mut reftable_iterator,
        name: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_next_ref(
        it: *mut reftable_iterator,
        ref_: *mut reftable_ref_record,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_seek_log_at(
        it: *mut reftable_iterator,
        name: *const ::std::os::raw::c_char,
        update_index: u64,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_seek_log(
        it: *mut reftable_iterator,
        name: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_next_log(
        it: *mut reftable_iterator,
        log: *mut reftable_log_record,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_iterator_destroy(it: *mut reftable_iterator);
    pub fn reftable_block_init(
        b: *mut reftable_block,
        source: *mut reftable_block_source,
        offset: u64,
        header_size: u32,
        table_block_size: u32,
        hash_size: u32,
        want_type: u8,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_block_release(b: *mut reftable_block);
    pub fn reftable_block_init_iterator(b: *const reftable_block, it: *mut reftable_iterator) -> ::std::os::raw::c_int;
    pub fn reftable_block_type(b: *const reftable_block) -> u8;
    pub fn reftable_block_first_key(b: *const reftable_block, key: *mut reftable_buf) -> ::std::os::raw::c_int;
    pub fn reftable_error_str(err: ::std::os::raw::c_int) -> *const ::std::os::raw::c_char;
    pub fn reftable_writer_new(
        out: *mut *mut reftable_writer,
        writer_func: ::std::option::Option<
            unsafe extern "C" fn(
                arg1: *mut ::std::os::raw::c_void,
                arg2: *const ::std::os::raw::c_void,
                arg3: usize,
            ) -> isize,
        >,
        flush_func: ::std::option::Option<
            unsafe extern "C" fn(arg1: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int,
        >,
        writer_arg: *mut ::std::os::raw::c_void,
        opts: *const reftable_write_options,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_writer_set_limits(w: *mut reftable_writer, min: u64, max: u64) -> ::std::os::raw::c_int;
    pub fn reftable_writer_add_ref(w: *mut reftable_writer, ref_: *mut reftable_ref_record) -> ::std::os::raw::c_int;
    pub fn reftable_writer_add_refs(
        w: *mut reftable_writer,
        refs: *mut reftable_ref_record,
        n: usize,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_writer_add_log(w: *mut reftable_writer, log: *mut reftable_log_record) -> ::std::os::raw::c_int;
    pub fn reftable_writer_add_logs(
        w: *mut reftable_writer,
        logs: *mut reftable_log_record,
        n: usize,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_writer_close(w: *mut reftable_writer) -> ::std::os::raw::c_int;
    pub fn reftable_writer_stats(w: *mut reftable_writer) -> *const reftable_stats;
    pub fn reftable_writer_free(w: *mut reftable_writer);
    pub fn reftable_new_stack(
        dest: *mut *mut reftable_stack,
        dir: *const ::std::os::raw::c_char,
        opts: *const reftable_write_options,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_next_update_index(st: *mut reftable_stack) -> u64;
    pub fn reftable_stack_new_addition(
        dest: *mut *mut reftable_addition,
        st: *mut reftable_stack,
        flags: ::std::os::raw::c_uint,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_addition_add(
        add: *mut reftable_addition,
        write_table: ::std::option::Option<
            unsafe extern "C" fn(wr: *mut reftable_writer, arg: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int,
        >,
        arg: *mut ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_addition_commit(add: *mut reftable_addition) -> ::std::os::raw::c_int;
    pub fn reftable_addition_destroy(add: *mut reftable_addition);
    pub fn reftable_stack_add(
        st: *mut reftable_stack,
        write_table: ::std::option::Option<
            unsafe extern "C" fn(
                wr: *mut reftable_writer,
                write_arg: *mut ::std::os::raw::c_void,
            ) -> ::std::os::raw::c_int,
        >,
        write_arg: *mut ::std::os::raw::c_void,
        flags: ::std::os::raw::c_uint,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_init_ref_iterator(
        st: *mut reftable_stack,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_init_log_iterator(
        st: *mut reftable_stack,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_merged_table(st: *mut reftable_stack) -> *mut reftable_merged_table;
    pub fn reftable_stack_destroy(st: *mut reftable_stack);
    pub fn reftable_stack_reload(st: *mut reftable_stack) -> ::std::os::raw::c_int;
    pub fn reftable_stack_compact_all(
        st: *mut reftable_stack,
        config: *mut reftable_log_expiry_config,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_compaction_required(
        st: *mut reftable_stack,
        use_heuristics: bool,
        required: *mut bool,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_auto_compact(st: *mut reftable_stack) -> ::std::os::raw::c_int;
    pub fn reftable_stack_clean(st: *mut reftable_stack) -> ::std::os::raw::c_int;
    pub fn reftable_stack_read_ref(
        st: *mut reftable_stack,
        refname: *const ::std::os::raw::c_char,
        ref_: *mut reftable_ref_record,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_read_log(
        st: *mut reftable_stack,
        refname: *const ::std::os::raw::c_char,
        log: *mut reftable_log_record,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_stack_compaction_stats(st: *mut reftable_stack) -> *mut reftable_compaction_stats;
    pub fn reftable_stack_hash_id(st: *mut reftable_stack) -> reftable_hash;
    pub fn reftable_fsck_check(
        stack: *mut reftable_stack,
        report_fn: reftable_fsck_report_fn,
        verbose_fn: reftable_fsck_verbose_fn,
        cb_data: *mut ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_merged_table_new(
        dest: *mut *mut reftable_merged_table,
        tables: *mut *mut reftable_table,
        n: usize,
        hash_id: reftable_hash,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_merged_table_init_ref_iterator(
        mt: *mut reftable_merged_table,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_merged_table_init_log_iterator(
        mt: *mut reftable_merged_table,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_merged_table_max_update_index(mt: *mut reftable_merged_table) -> u64;
    pub fn reftable_merged_table_min_update_index(mt: *mut reftable_merged_table) -> u64;
    pub fn reftable_merged_table_free(m: *mut reftable_merged_table);
    pub fn reftable_merged_table_hash_id(m: *mut reftable_merged_table) -> reftable_hash;
    pub fn reftable_table_new(
        out: *mut *mut reftable_table,
        src: *mut reftable_block_source,
        name: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_table_incref(table: *mut reftable_table);
    pub fn reftable_table_decref(table: *mut reftable_table);
    pub fn reftable_table_init_ref_iterator(
        t: *mut reftable_table,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_table_init_log_iterator(
        t: *mut reftable_table,
        it: *mut reftable_iterator,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_table_hash_id(t: *mut reftable_table) -> reftable_hash;
    pub fn reftable_table_refs_for(
        t: *mut reftable_table,
        it: *mut reftable_iterator,
        oid: *mut u8,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_table_max_update_index(t: *mut reftable_table) -> u64;
    pub fn reftable_table_min_update_index(t: *mut reftable_table) -> u64;
    pub fn reftable_table_iterator_init(
        it: *mut reftable_table_iterator,
        t: *mut reftable_table,
    ) -> ::std::os::raw::c_int;
    pub fn reftable_table_iterator_release(it: *mut reftable_table_iterator);
    pub fn reftable_table_iterator_next(
        it: *mut reftable_table_iterator,
        out: *mut *const reftable_block,
    ) -> ::std::os::raw::c_int;
}
