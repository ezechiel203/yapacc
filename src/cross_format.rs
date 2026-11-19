//! Cross-format / cross-distro subsystem.
//! Long-term goal: unified model over arch/deb/rpm/flatpak/snap/appimage and
//! mediated installs via conversion or containers with strict risk gates.
//! Flatpak host adapter is implemented (search/info/list + gated install/remove/update);
//! container (distrobox) mediation planning is implemented here; other
//! non-native formats say that no command handles them.

#[derive(Clone, Debug)]
pub enum PkgFormat {
    Arch,
    Deb,
    Rpm,
    Flatpak,
    Snap,
    AppImage,
    Unknown(#[allow(dead_code)] String), // rendered via Debug in plans
}

#[derive(Clone, Debug)]
pub struct PackageRef {
    pub name: String,
    pub format: PkgFormat,
    pub source: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CapabilityReport {
    pub format: String,
    pub host_supported: bool,
    pub install_supported: bool,
    pub notes: String,
}

pub fn detect_host_capabilities() -> Vec<CapabilityReport> {
    let mut caps = Vec::new();
    let has = |c: &str| crate::util::have(c);
    caps.push(CapabilityReport {
        format: "arch/pacman".into(),
        host_supported: has("pacman"),
        install_supported: has("pacman"),
        notes: "Native Arch backend (primary).".into(),
    });
    caps.push(CapabilityReport {
        format: "aur/yay".into(),
        host_supported: has("yay"),
        install_supported: has("yay"),
        notes: "AUR helper integration for build/install/search flows.".into(),
    });
    caps.push(CapabilityReport{ format:"flatpak".into(), host_supported: has("flatpak"), install_supported: has("flatpak"), notes:"Sandboxed app backend: search/info/list and gated install/remove/update implemented via flatpak CLI.".into() });
    caps.push(CapabilityReport{ format:"snap".into(), host_supported: has("snap") || has("unsquashfs"), install_supported: has("snap") || has("unsquashfs"), notes: if has("snap") { "snapd is present, but yapacc does not use it: a snap file is unpacked with unsquashfs into an isolated directory (no confinement, no snapd integration), tracked in the ledger.".into() } else { "User-scope squashfs payload extract to managed prefix via unsquashfs (no confinement/snapd integration), tracked in ledger; not seamless snapd install.".into() } });
    caps.push(CapabilityReport{ format:"appimage".into(), host_supported: true, install_supported: true, notes:"User-scope AppImage install/remove from local path or URL, tracked in user ledger (home-only).".into() });
    caps.push(CapabilityReport{ format:"deb".into(), host_supported: has("ar")&&has("bsdtar"), install_supported: has("ar")&&has("bsdtar"), notes:"User-scope payload extract to managed prefix (no scriptlets/deps), tracked in ledger; not seamless system integration.".into() });
    caps.push(CapabilityReport{ format:"rpm".into(), host_supported: has("bsdtar")||has("rpm2cpio"), install_supported: has("bsdtar")||has("rpm2cpio"), notes:"User-scope payload extract to managed prefix (no scriptlets/deps), tracked in ledger; not seamless system integration.".into() });
    caps
}

pub fn plan_cross_format_install(pref: &PackageRef) -> String {
    let head = |format: &str| {
        format!(
            "CROSS-FORMAT INSTALL PLAN\nRequested: {} ({format}) from {}\n\n",
            pref.name, pref.source
        )
    };
    match pref.format {
        PkgFormat::Flatpak => {
            head("flatpak")
                + "Handled through the flatpak command of this host.\n- Read: yapacc flatpak search QUERY | flatpak list | flatpak info APP\n- Plan: yapacc plan flatpak APP | plan flatpak-remove APP | plan flatpak-update [APP]\n- Change (gated): yapacc install flatpak APP | remove flatpak APP | update flatpak [APP]\n\nA flatpak change installs or removes applications and runtimes; add --dryrun to see the command without running it."
        }
        PkgFormat::AppImage => {
            head("appimage")
                + "Handled as a file under your home directory.\n- Plan: yapacc plan appimage --name NAME --source FILE-OR-URL\n- Install (gated): yapacc install appimage --name NAME --source FILE-OR-URL [--sha256 HEX]\n- List: yapacc external list\n- Remove (gated): yapacc remove appimage --id ID\n\nThe file is copied under your home directory, made executable and recorded in the user ledger; the package manager is not involved. Verified: only the SHA-256 you pass with --sha256 (a mismatch deletes the file, exit 5). NOT verified without it: anything about the file; the output says so. No publisher signature is checked in either case. A plain http:// source is refused unless --allow-http is given."
        }
        PkgFormat::Deb => {
            head("deb")
                + "Handled as an isolated install: the files are unpacked into a directory of their own, not installed into the system.\n- Plan: yapacc plan foreign --name NAME --format deb --source FILE.deb\n- Install (gated): yapacc install foreign --name NAME --format deb --source FILE.deb\n- Remove (gated): yapacc remove foreign --id ID\n\nOnly the files are unpacked: no maintainer script is run, and the install is recorded in the ledger. Dependencies, library compatibility and services are not handled."
        }
        PkgFormat::Rpm => {
            head("rpm")
                + "Handled as an isolated install: the files are unpacked into a directory of their own, not installed into the system.\n- Plan: yapacc plan foreign --name NAME --format rpm --source FILE.rpm\n- Install (gated): yapacc install foreign --name NAME --format rpm --source FILE.rpm\n- Remove (gated): yapacc remove foreign --id ID\n\nOnly the files are unpacked: no scriptlet is run, and the install is recorded in the ledger. Dependencies, library compatibility and services are not handled."
        }
        PkgFormat::Snap => {
            head("snap")
                + "Handled as an isolated install: the squashfs is unpacked with unsquashfs into a directory of its own. This is not a snapd install.\n- Read: yapacc foreign info --format snap --source FILE.snap\n- Plan: yapacc plan foreign --name NAME --format snap --source FILE.snap\n- Install (gated): yapacc install foreign --name NAME --format snap --source FILE.snap\n- Remove, update (gated): yapacc remove foreign --id ID | update foreign --id ID --source FILE.snap\n\nNo confinement, no services and no snapd: the files are unpacked and the programs found are linked under attributed names."
        }
        _ => format!(
            "CROSS-FORMAT INSTALL PLAN\nRequested: {} ({:?}) from {}\n\nNo yapacc command installs a package of this format. Formats handled: flatpak, appimage, and deb, rpm, snap as isolated installs (`yapacc plan cross NAME --format FORMAT` describes each). A package of this system's own format goes through `yapacc install repo`.",
            pref.name, pref.format, pref.source
        ),
    }
}

/// Container mediation: the recommended strategy for running foreign-distro
/// software WITH its real dependencies (as opposed to extract-only inspection).
#[derive(Clone, Debug, serde::Serialize)]
pub struct ContainerMediation {
    pub available: bool,
    pub engine: String, // distrobox | podman | docker | toolbox | none
    pub name: String,
    pub image: String,
    pub commands: Vec<String>,
    pub notes: Vec<String>,
}

fn which(cmd: &str) -> bool {
    crate::util::have(cmd)
}

pub fn container_mediation_plan(image: &str, name: &str) -> ContainerMediation {
    let mut m = ContainerMediation {
        available: false,
        engine: "none".into(),
        name: name.to_string(),
        image: image.to_string(),
        commands: vec![],
        notes: vec![],
    };
    if which("distrobox") {
        m.available = true;
        m.engine = "distrobox".into();
        m.commands
            .push(format!("distrobox create --name {name} --image {image}"));
        m.commands.push(format!(
            "distrobox enter {name}   # then use the distro's native package manager inside"
        ));
        m.commands.push(
            "distrobox-export --app <app>   # expose an app from the container to the host"
                .to_string(),
        );
        m.notes.push("distrobox integrates home dirs, GUI apps, and binaries; the safest mediation for foreign-distro software.".into());
    } else if which("toolbox") {
        m.available = true;
        m.engine = "toolbox".into();
        m.commands
            .push(format!("toolbox create {name} --image {image}"));
        m.commands.push(format!("toolbox enter {name}"));
        m.notes
            .push("toolbox is Silverblue-oriented; distrobox offers broader image support.".into());
    } else if which("podman") || which("docker") {
        let eng = if which("podman") { "podman" } else { "docker" };
        m.available = true;
        m.engine = eng.into();
        m.commands
            .push(format!("{eng} run -it --name {name} {image}"));
        m.notes.push("plain containers isolate fully (no home/GUI integration); consider installing distrobox for better mediation.".into());
    } else {
        m.notes.push("no container engine found; install distrobox + podman for the recommended mediation path".into());
        m.notes.push("extract-only isolated installs (`yapacc install foreign`) remain available for inspection, with attributed bin names".into());
    }
    m
}

/// Container names and image references are passed to the engine as
/// separate arguments (never through a shell) and must look like what
/// they claim to be.
fn valid_container_arg(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':' | b'@')
        })
}

/// Gated container creation via the detected engine.
pub fn container_create(m: &ContainerMediation) -> Result<String, String> {
    if !valid_container_arg(&m.name) || !valid_container_arg(&m.image) {
        return Err(format!(
            "refusing container name/image with unexpected characters: name='{}' image='{}'",
            m.name, m.image
        ));
    }
    match m.engine.as_str() {
        "distrobox" => {
            let out = std::process::Command::new("distrobox")
                .args(["create", "--name", &m.name, "--image", &m.image])
                .output()
                .map_err(|e| format!("failed to run distrobox create: {e}"))?;
            if out.status.success() {
                Ok(format!("container created: {} ({})", m.name, m.image))
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
        other => Err(format!(
            "gated creation not implemented for engine '{other}'; run: {}",
            m.commands
                .first()
                .map(String::as_str)
                .unwrap_or("(no command)")
        )),
    }
}
