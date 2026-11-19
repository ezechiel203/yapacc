// yapacc — Yet Another PACkage Companion
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Shared library behind the yapacc binaries.
//!
//! The project ships several small executables instead of one large one
//! (see docs/ARCHITECTURE.md): `yapacc` (core CLI), `yapacc-tui`
//! (interactive interface) and `yapacc-mcp` (AI interface). Each binary is
//! optional; the core never requires the others.

#[cfg(feature = "alpm")]
pub mod alpm_backend;
#[cfg(not(feature = "alpm"))]
#[path = "alpm_stub.rs"]
pub mod alpm_backend;
#[cfg(feature = "alpm")]
pub mod alpm_flows;
#[cfg(not(feature = "alpm"))]
#[path = "alpm_flows_stub.rs"]
pub mod alpm_flows;

pub mod appimage;
pub mod aur_rpc;
pub mod backend;
pub mod brew;
pub mod briefing;
pub mod clean;
pub mod cli;
pub mod config;
pub mod configs;
pub mod cross_format;
pub mod docs;
pub mod doctor;
pub mod foreign_extract;
pub mod ledger;
pub mod mcp;
pub mod names;
pub mod ops;
pub mod plan;
pub mod plugin;
pub mod prompt;
pub mod recipe;
pub mod repair;
pub mod resolve;
pub mod restart;
pub mod safety;
pub mod sandbox;
pub mod snapshot;
pub mod source_repos;
pub mod taxonomy;
pub mod timeline;
#[cfg(feature = "tui")]
pub mod tui;
pub mod undo;
pub mod update_service;
pub mod why;

pub use error::{Error, Kind};
pub use octo_head::pkglog;
pub use octo_head::{error, native, platform, util, vercmp};
