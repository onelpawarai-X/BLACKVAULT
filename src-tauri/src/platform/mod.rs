//! BLACKVAULT — Windows integration.
//!
//! Every module here has a real Windows implementation behind `#[cfg(windows)]`
//! and an explicit `Unsupported` stub elsewhere, so the crate always builds and
//! the pure logic stays testable on any host. The stubs never pretend to have
//! done the work: they return an error the UI surfaces.

pub mod applock;
pub mod automation;
pub mod fileprot;
pub mod hello;
pub mod launcher;
pub mod lock;
pub mod service;

pub use applock::*;
pub use automation::*;
pub use fileprot::*;
pub use hello::*;
pub use launcher::*;
pub use lock::*;
pub use service::*;
