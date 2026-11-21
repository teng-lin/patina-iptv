//! The native implementations of the runtime's ports, and the services only
//! native hosts have. Everything here is behind the default `native`
//! feature: a browser build supplies its ports from the host instead.
pub(crate) mod network;
