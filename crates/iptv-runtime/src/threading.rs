//! Thread-safety bounds for native workers and cooperative browser tasks.

/// `Send` when the runtime runs work on threads (the `native` feature); no
/// bound in a single-threaded browser build, whose adapters hold the page's
/// values.
#[cfg(feature = "native")]
pub trait MaybeSend: Send {}
#[cfg(feature = "native")]
impl<T: Send + ?Sized> MaybeSend for T {}
/// `Send` when the runtime runs work on threads (the `native` feature); no
/// bound in a single-threaded browser build, whose adapters hold the page's
/// values.
#[cfg(not(feature = "native"))]
pub trait MaybeSend {}
#[cfg(not(feature = "native"))]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` exactly when [`MaybeSend`] is `Send`.
#[cfg(feature = "native")]
pub trait MaybeSync: Sync {}
#[cfg(feature = "native")]
impl<T: Sync + ?Sized> MaybeSync for T {}
/// `Sync` exactly when [`MaybeSend`] is `Send`.
#[cfg(not(feature = "native"))]
pub trait MaybeSync {}
#[cfg(not(feature = "native"))]
impl<T: ?Sized> MaybeSync for T {}
