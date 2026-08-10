//! The Onda language server shared by native stdio and browser Wasm hosts.
//!
//! `onda lsp` supplies the stdio transport; browser hosts feed the same
//! server JSON-RPC messages through the Wasm package.

pub mod formatting;
mod server;
pub mod stdlib_docs;

pub use server::{run_stdio_loop, LspSession};
