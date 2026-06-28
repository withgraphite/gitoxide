#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

pub use gix_reftable_sys::*;

pub const HASH_SIZE_MAX: usize = REFTABLE_HASH_SIZE_MAX as usize;

pub const REF_DELETION: reftable_ref_record__bindgen_ty_1 = reftable_ref_record_REFTABLE_REF_DELETION;
pub const REF_VAL1: reftable_ref_record__bindgen_ty_1 = reftable_ref_record_REFTABLE_REF_VAL1;
pub const REF_VAL2: reftable_ref_record__bindgen_ty_1 = reftable_ref_record_REFTABLE_REF_VAL2;
pub const REF_SYMREF: reftable_ref_record__bindgen_ty_1 = reftable_ref_record_REFTABLE_REF_SYMREF;
pub const LOG_DELETION: reftable_log_record__bindgen_ty_1 = reftable_log_record_REFTABLE_LOG_DELETION;
pub const LOG_UPDATE: reftable_log_record__bindgen_ty_1 = reftable_log_record_REFTABLE_LOG_UPDATE;

pub const IO_ERROR: std::ffi::c_int = reftable_error_REFTABLE_IO_ERROR;
pub const FORMAT_ERROR: std::ffi::c_int = reftable_error_REFTABLE_FORMAT_ERROR;
pub const NOT_EXIST_ERROR: std::ffi::c_int = reftable_error_REFTABLE_NOT_EXIST_ERROR;
pub const LOCK_ERROR: std::ffi::c_int = reftable_error_REFTABLE_LOCK_ERROR;
pub const API_ERROR: std::ffi::c_int = reftable_error_REFTABLE_API_ERROR;
pub const ZLIB_ERROR: std::ffi::c_int = reftable_error_REFTABLE_ZLIB_ERROR;
pub const EMPTY_TABLE_ERROR: std::ffi::c_int = reftable_error_REFTABLE_EMPTY_TABLE_ERROR;
pub const REFNAME_ERROR: std::ffi::c_int = reftable_error_REFTABLE_REFNAME_ERROR;
pub const ENTRY_TOO_BIG_ERROR: std::ffi::c_int = reftable_error_REFTABLE_ENTRY_TOO_BIG_ERROR;
pub const OUTDATED_ERROR: std::ffi::c_int = reftable_error_REFTABLE_OUTDATED_ERROR;
pub const OUT_OF_MEMORY_ERROR: std::ffi::c_int = reftable_error_REFTABLE_OUT_OF_MEMORY_ERROR;

pub const ADDITION_RELOAD: u32 = REFTABLE_STACK_NEW_ADDITION_RELOAD;

pub type Stack = reftable_stack;
pub type RefIterator = reftable_iterator;
pub type LogIterator = reftable_iterator;
pub type Addition = reftable_addition;
pub type Writer = reftable_writer;
