#![cfg(windows)]
#![feature(
    maybe_uninit_uninit_array,
    maybe_uninit_slice,
    maybe_uninit_array_assume_init,
    once_cell,
    io_safety,
    linked_list_cursors
)]
#![warn(
    unsafe_op_in_unsafe_fn,
    missing_docs,
    missing_debug_implementations,
    missing_copy_implementations,
    clippy::missing_const_for_fn,
    rust_2018_idioms,
    clippy::todo,
    clippy::manual_assert,
    clippy::must_use_candidate,
    clippy::inconsistent_struct_constructor,
    clippy::wrong_self_convention,
    rustdoc::broken_intra_doc_links,
    rustdoc::private_intra_doc_links
)]
#![allow(
    clippy::module_inception,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::borrow_as_ptr
)]
#![cfg_attr(feature = "remote_procedure", doc = include_str!("../crate-doc.md"))]
#![cfg_attr(not(feature = "remote_procedure"), allow(missing_docs))]
#![cfg_attr(feature = "doc_cfg", feature(doc_cfg))]

mod syringe;
pub use syringe::*;

mod process;
pub use process::*;

mod process_ref;
pub use process_ref::*;

mod process_module;
pub use process_module::*;

#[cfg(any(feature = "remote_procedure", feature = "doc_cfg"))]
#[cfg_attr(feature = "doc_cfg", doc(cfg(feature = "remote_procedure")))]
mod remote_procedure;
#[cfg(any(feature = "remote_procedure", feature = "doc_cfg"))]
pub use remote_procedure::*;

#[cfg_attr(not(feature = "process_memory"), allow(dead_code))]
#[cfg(any(feature = "process_memory", feature = "doc_cfg"))]
#[cfg_attr(feature = "doc_cfg", doc(cfg(feature = "process_memory")))]
/// Module containing utilities for dealing with memory of another process.
pub mod process_memory;
#[cfg(not(any(feature = "process_memory", feature = "doc_cfg")))]
/// Module containing utilities for dealing with memory of another process.
pub(crate) mod process_memory;

pub(crate) mod utils;

/// Module containing the error enums used in this crate.
pub mod error;
