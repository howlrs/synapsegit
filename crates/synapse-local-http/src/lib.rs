//! Loopback-only HTTP and UI transport for the SynapseGit local application.

#![forbid(unsafe_code)]

mod app;
mod handlers;
mod problem;
mod security;
mod staging;
mod state;
mod templates;
mod views;

#[cfg(test)]
mod tests;

pub use app::{LocalHttpApplication, StartupError, build_local_application};
