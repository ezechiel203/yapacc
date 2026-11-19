//! octo-head — the code shared by the yapacc binaries: the platform probe,
//! the native package-manager backends, the package-log parsers and the
//! typed errors. A library, never a product (docs/ARCHITECTURE.md).

pub mod error;
pub mod native;
pub mod pkglog;
pub mod platform;
pub mod util;
pub mod vercmp;
