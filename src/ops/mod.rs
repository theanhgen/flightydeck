//! Operations shared by the CLI and MCP. Each takes `&Ctx` and plain args, returns a
//! `Render`able value. Mutating ops call `ctx.guard(..)` first.

pub mod about;
pub mod connections;
pub mod flights;
pub mod friends;
pub mod ics;
pub mod reference;
pub mod stats;
pub mod status;
pub mod write;
