//! Trusted transport-neutral read facade for the SynapseGit localhost application.
//!
//! This crate owns the exact startup project catalog and the versioned DTOs
//! consumed by transports. It intentionally exposes no write primitive and
//! retains repository paths only inside the service boundary.

#![forbid(unsafe_code)]

mod catalog;
mod dto;
mod service;

pub use catalog::{CatalogError, ProjectRegistration};
pub use dto::*;
pub use service::{
    IMAGE_RESPONSE_MAX_BYTES, LocalService, MAX_CREATOR_SESSIONS, MAX_PROJECTS, MAX_REFS,
    ServiceError,
};
