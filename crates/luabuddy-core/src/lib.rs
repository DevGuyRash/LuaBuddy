//! Host-independent configuration, resolution, and lifecycle policy for LuaBuddy.

pub mod diagnostics;
pub mod lifecycle;
pub mod management;
pub mod profile;
pub mod resolve;
pub mod selection;
pub mod source;

pub use profile::{CoreError, ModId, Profile, ReplayEvent, read_profiles};
pub use resolve::{NativeMetadata, Resolution, ResolvedProfile, SkipReason, resolve};
