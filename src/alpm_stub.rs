// yapacc — libalpm reads, stub for builds without the `alpm` feature
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! Same surface as `alpm_backend.rs`; every call reports that native ALPM
//! support was not compiled in. This keeps the core buildable on hosts
//! without libalpm (other distributions, minimal CI runners).

const MSG: &str =
    "this build of yapacc has no native libalpm support (built without the `alpm` feature)";

#[derive(Clone, Debug)]
pub struct AlpmPkg {
    pub name: String,
    pub version: String,
    pub desc: String,
    pub installed_size: i64,
    pub reason: String,
    pub required_by: Vec<String>,
}

pub fn list_local() -> Result<Vec<AlpmPkg>, String> {
    Err(MSG.into())
}

pub fn info(_name: &str) -> Result<Option<AlpmPkg>, String> {
    Err(MSG.into())
}

pub fn list_leaves() -> Result<Vec<AlpmPkg>, String> {
    Err(MSG.into())
}
