use alpm::{Alpm, PackageReason, SigLevel};
use anyhow::{Context, Result};

fn open() -> Result<Alpm> {
    // Standard Arch paths
    let handle = Alpm::new("/", "/var/lib/pacman/").context("failed to initialize libalpm")?;
    // Register sync dbs from config is complex; for local queries not strictly needed.
    // Try to register common dbs best-effort for completeness (ignore errors).
    for db in ["core", "extra", "multilib"] {
        let _ = handle.register_syncdb(db, SigLevel::USE_DEFAULT);
    }
    Ok(handle)
}

#[derive(Clone, Debug)]
pub struct AlpmPkg {
    pub name: String,
    pub version: String,
    pub desc: String,
    pub installed_size: i64,
    pub reason: String,
    pub required_by: Vec<String>,
}

pub fn list_local() -> Result<Vec<AlpmPkg>> {
    let h = open()?;
    let db = h.localdb();
    let mut out = Vec::new();
    for pkg in db.pkgs() {
        let reason = match pkg.reason() {
            PackageReason::Explicit => "explicit",
            PackageReason::Depend => "dependency",
        }
        .to_string();
        let required_by = pkg.required_by().iter().map(|s| s.to_string()).collect();
        out.push(AlpmPkg {
            name: pkg.name().to_string(),
            version: pkg.version().as_str().to_string(),
            desc: pkg.desc().unwrap_or("").to_string(),
            installed_size: pkg.isize(),
            reason,
            required_by,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn info(name: &str) -> Result<Option<AlpmPkg>> {
    let h = open()?;
    let db = h.localdb();
    let Some(pkg) = db.pkg(name).ok() else {
        return Ok(None);
    };
    let reason = match pkg.reason() {
        PackageReason::Explicit => "explicit",
        PackageReason::Depend => "dependency",
    }
    .to_string();
    let required_by = pkg.required_by().iter().map(|s| s.to_string()).collect();
    Ok(Some(AlpmPkg {
        name: pkg.name().to_string(),
        version: pkg.version().as_str().to_string(),
        desc: pkg.desc().unwrap_or("").to_string(),
        installed_size: pkg.isize(),
        reason,
        required_by,
    }))
}

pub fn list_leaves() -> Result<Vec<AlpmPkg>> {
    let mut pkgs = list_local()?;
    pkgs.retain(|p| p.reason == "explicit" && p.required_by.is_empty());
    Ok(pkgs)
}
