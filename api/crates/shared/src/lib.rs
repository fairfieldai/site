//! Code shared by the site API and the Discord functions.

mod dynamo;
pub mod events;
pub mod links;
pub mod ssm;
pub mod subscribers;
pub mod time;

/// Error type for the AWS and HTTP calls in this crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
