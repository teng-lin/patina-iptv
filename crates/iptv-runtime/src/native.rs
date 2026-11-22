//! The native implementations of the runtime's ports, and the services only
//! native hosts have. Everything here is behind the default `native`
//! feature: a browser build supplies its ports from the host instead.
pub(crate) mod execution;
pub(crate) mod network;

/// The operating system's secure random source ([`crate::Ports::entropy`]).
pub(crate) fn entropy(bytes: &mut [u8]) -> std::io::Result<()> {
    getrandom::fill(bytes).map_err(|error| std::io::Error::other(error.to_string()))
}
