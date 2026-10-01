//! flightydeck: read your Flighty flights and add flights to your account.
//! Unofficial; not made by or affiliated with Flighty.
//!
//! Both front-ends (`flightydeck` CLI and `flightydeck mcp`) call only `ops::*`.

pub mod api;
pub mod creds;
pub mod ctx;
pub mod db;
pub mod error;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod owner;
pub mod proto;
pub mod render;
pub mod time;

pub use ctx::Ctx;
pub use error::{Error, Result};
