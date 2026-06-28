//! Raw, unsafe bindings to Git's vendored reftable C library.

#![allow(
    clippy::all,
    clippy::pedantic,
    dead_code,
    improper_ctypes,
    missing_docs,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unsafe_code,
    unsafe_op_in_unsafe_fn
)]

use libz_sys as _;

#[cfg(feature = "rust-platform")]
mod platform;

include!("bindings.rs");
