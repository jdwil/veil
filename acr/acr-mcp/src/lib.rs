//! ACR - MCP server exposing ACR capabilities over HTTP.

pub mod handlers;
pub mod server;
pub mod types;

pub use server::build_router;
