//! Trusted transport-neutral facade for the SynapseGit localhost application.
//!
//! This crate owns the exact startup project catalog and the versioned DTOs
//! consumed by transports. Creator writes are limited to catalog-fixed
//! begin/decide use cases, and repository paths remain inside the service
//! boundary.

#![forbid(unsafe_code)]

mod catalog;
mod dto;
mod service;

pub use catalog::{CatalogError, ProjectRegistration};
pub use dto::*;
pub use service::{
    IMAGE_RESPONSE_MAX_BYTES, LocalService, MAX_CREATOR_SESSIONS, MAX_PENDING_CREATOR_SESSIONS,
    MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT, MAX_PROJECTS, MAX_REFS, ServiceError,
};
