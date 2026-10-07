//! Code shared by the site API and the Discord functions.

pub mod links;
pub mod ssm;

/// Error type for the AWS and HTTP calls in this crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
