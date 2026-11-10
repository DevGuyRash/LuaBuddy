#![deny(unsafe_op_in_unsafe_fn)]

//! Rust patch policy, dispatch, and the versioned extension ABI.
//!
//! A registry entry is not installed until a host backend acknowledges it.

pub mod extension;
mod registry;

pub use registry::*;
