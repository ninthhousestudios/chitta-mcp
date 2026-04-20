//! Tool handlers and shared argument validators.
//!
//! Each tool lives in its own submodule with `Args` + `Output` types
//! (`serde::Deserialize` / `Serialize`) and one async `handle` fn.

pub mod get;
pub mod search;
pub mod store;
pub mod validate;

pub use get::{GetArgs, GetOutput};
pub use search::{SearchArgs, SearchHit, SearchOutput};
pub use store::{StoreArgs, StoreOutput};
