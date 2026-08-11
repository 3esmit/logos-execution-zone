//! RPC Server Actor serves RPC queries and forwards them to Executor.

pub use actor::RpcServerActor;

pub mod actor;
pub mod error;
mod service;

pub type Result<T> = std::result::Result<T, error::Error>;
