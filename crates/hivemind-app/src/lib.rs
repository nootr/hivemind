//! Application use cases and ports for HIVEMIND.
//!
//! This crate owns orchestration, not infrastructure. It depends on the core
//! protocol types and talks to storage, identity and clocks through ports.

mod error;
mod ports;
mod publish_object;

pub use error::{AppError, AppResult};
pub use ports::{ChunkStorePort, ClockPort, IdentityPort, ObjectStorePort};
pub use publish_object::{PublishObject, PublishObjectInput, PublishObjectOutput};
