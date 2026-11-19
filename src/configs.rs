// yapacc — `configs`: configuration files left unmerged by upgrades
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! A report, not an action: package managers never overwrite a
//! configuration file you changed; they leave the other version next to it
//! (`.pacnew`, `.dpkg-dist`, `.rpmnew`, ...) and say so once, in the middle
//! of an upgrade. This module finds those files under `/etc`, says what
//! each one means, and prints the commands that compare and merge them.
//! It runs none of them and deletes or edits nothing.

use crate::error::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The walk stops after this many directories and says so.
pub const MAX_DIRS: usize = 20_000;

/// Files larger than this are not compared byte by byte.
const MAX_COMPARE: u64 = 8 * 1024 * 1024;

const NEW: &str = "the package's new default; your file was kept";
const OLD: &str = "your old file, saved; the package's version is live";
/// The same kind of file when the live file is gone (the package was
/// removed, or the file deleted): no version of it is live.
const OLD_ALONE: &str = "your old file, saved; the live file no longer exists";

/// What a pending file of `kind` is, given whether its live file exists.
/// A saved old file (`.pacsave`, `.rpmsave`, `.dpkg-old`) beside no live
/// file is not said to have "the package's version live". Pure.
pub fn meaning_of(kind: Kind, live_missing: bool) -> &'static str {
    if live_missing && kind.meaning == OLD {
        OLD_ALONE
    } else {
        kind.meaning
    }
}
const INTERRUPTED: &str =
    "the package's new file, left by an interrupted dpkg run; your file was kept";

/// One kind of pending file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub suffix: &'static str,
    /// what a file with this suffix is, in one phrase
    pub meaning: &'static str,
    /// which packaging family writes it: "pacman", "dpkg", "rpm" or "apk"
    pub family: &'static str,
}

const fn kind(suffix: &'static str, meaning: &'static str, family: &'static str) -> Kind {
    Kind {
        suffix,
        meaning,
        family,
    }
}

pub const KINDS: [Kind; 9] = [
    kind(".pacnew", NEW, "pacman"),
    kind(".pacsave", OLD, "pacman"),
    kind(".dpkg-dist", NEW, "dpkg"),
    kind(".dpkg-new", INTERRUPTED, "dpkg"),
    kind(".dpkg-old", OLD, "dpkg"),
    kind(".ucf-dist", NEW, "dpkg"),
    kind(".rpmnew", NEW, "rpm"),
    kind(".rpmsave", OLD, "rpm"),
    kind(".apk-new", NEW, "apk"),
];

/// Which kind of pending file `file_name` is, and the name of the live
/// file it belongs to. `None` for every other file.
pub fn classify(file_name: &str) -> Option<(Kind, &str)> {
    KINDS.iter().find_map(|k| {
        let live = file_name.strip_suffix(k.suffix)?;
        (!live.is_empty()).then_some((*k, live))
    })
}

/// The merge tool of a packaging family, when it is installed.
pub fn merge_tool(family: &str, have: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    let (tool, command) = match family {
        "pacman" => ("pacdiff", "pacdiff -s"),
        "rpm" => ("rpmconf", "sudo rpmconf -a"),
        "apk" => ("update-conf", "sudo update-conf"),
        _ => return None,
    };
    have(tool).then_some(command)
}

/// `s` with control characters made visible (`\n`, `\x1b`): a file name is
/// attacker-controlled text and must not move the cursor, recolour the
/// terminal or fake a line of this report.
pub fn visible(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32 & 0xff)),
            c => out.push(c),
        }
    }
    out
}

/// Quote `s` for a POSIX shell when it holds anything a shell would touch.
/// A name with control characters is written `$'..\n..'` (bash, zsh, ksh)
/// so the line stays one printable line.
fn sh_quote(s: &str) -> String {
    if s.chars().any(char::is_control) {
        let esc = visible(&s.replace('\\', "\\\\").replace('\'', "\\'"));
        return format!("$'{esc}'");
    }
    let plain = !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'.' | b'_' | b'-' | b'/' | b'+' | b'@' | b':' | b',')
        });
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// What to do with a pending file, in one sentence.
pub fn advice(live_missing: bool, identical: Option<bool>) -> &'static str {
    match (live_missing, identical) {
        (true, _) => {
            "the live file is missing: the package was removed or the file deleted; nothing to merge, keep or delete the pending file as you see fit"
        }
        (false, Some(true)) => "identical: safe to delete the pending file",
        (false, Some(false)) => {
            "the files differ: compare them, merge what you want, then delete the pending file"
        }
        (false, None) => {
            "could not be compared (not readable as this user, or too large): run the diff with sudo"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pending {
    /// the file the upgrade left behind
    pub pending: String,
    /// the configuration file it belongs to
    pub live: String,
    pub suffix: &'static str,
    pub meaning: &'static str,
    /// "YYYY-MM-DD" (UTC) of the last modification; `None`: not readable
    pub pending_date: Option<String>,
    pub live_date: Option<String>,
    pub live_missing: bool,
    /// `None`: the two files could not be compared
    pub identical: Option<bool>,
    /// package that owns the live file; `None`: not owned, or not known
    pub owner: Option<String>,
    pub advice: &'static str,
    /// the exact commands that compare and merge; none of them is run here
    pub commands: Vec<String>,
}

fn date_of(meta: &std::fs::Metadata) -> Option<String> {
    let secs = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    // fmt_utc gives "YYYY-MM-DD HH:MM UTC"
    Some(crate::util::fmt_utc(secs).chars().take(10).collect())
}

/// Are the two files byte for byte the same? `None` when that could not
/// be established.
fn same_bytes(a: &Path, a_len: u64, b: &Path, b_len: u64) -> Option<bool> {
    if a_len != b_len {
        return Some(false);
    }
    if a_len > MAX_COMPARE {
        return None;
    }
    let left = std::fs::read(a).ok()?;
    let right = std::fs::read(b).ok()?;
    Some(left == right)
}

/// Everything the report says about one pending file. `owner` answers
/// "which package owns this path" and `have` "is this tool installed";
/// `None` when `pending` does not carry one of the known suffixes.
pub fn describe(
    pending: &Path,
    owner: &dyn Fn(&str) -> Option<String>,
    have: &dyn Fn(&str) -> bool,
) -> Option<Pending> {
    let pending_text = pending.to_string_lossy().to_string();
    let file_name = pending.file_name()?.to_string_lossy().to_string();
    let (kind, live_name) = classify(&file_name)?;
    let live = pending.with_file_name(live_name);
    let live_text = live.to_string_lossy().to_string();

    let pending_meta = std::fs::metadata(pending).ok();
    let live_meta = std::fs::metadata(&live).ok();
    let live_missing = matches!(
        std::fs::symlink_metadata(&live),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    );
    let identical = match (&pending_meta, &live_meta) {
        (Some(p), Some(l)) if l.is_file() => same_bytes(&live, l.len(), pending, p.len()),
        _ => None,
    };

    let mut commands: Vec<String> = Vec::new();
    if !live_missing {
        let sudo = if identical.is_none() { "sudo " } else { "" };
        commands.push(format!(
            "{sudo}diff -u {} {}",
            sh_quote(&live_text),
            sh_quote(&pending_text)
        ));
        if identical != Some(true)
            && let Some(tool) = merge_tool(kind.family, have)
        {
            commands.push(tool.to_string());
        }
    }
    Some(Pending {
        owner: if live_missing {
            None
        } else {
            owner(&live_text)
        },
        pending: pending_text,
        live: live_text,
        suffix: kind.suffix,
        meaning: meaning_of(kind, live_missing),
        pending_date: pending_meta.as_ref().and_then(date_of),
        live_date: live_meta.as_ref().and_then(date_of),
        live_missing,
        identical,
        advice: advice(live_missing, identical),
        commands,
    })
}

/// What walking a tree found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// pending files, sorted
    pub files: Vec<PathBuf>,
    pub dirs: usize,
    /// directories that could not be read (their content is not covered)
    pub unreadable_dirs: usize,
    /// the walk stopped at the directory limit
    pub truncated: bool,
}

/// Walk `root` depth-first for pending files. Symbolic links are not
/// followed; at most `max_dirs` directories are opened.
pub fn scan(root: &Path, max_dirs: usize) -> Scan {
    let mut found = Scan::default();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if found.dirs >= max_dirs {
            found.truncated = true;
            break;
        }
        found.dirs += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            found.unreadable_dirs += 1;
            continue;
        };
        for entry in entries.flatten() {
            // file_type() describes the entry itself: a link is a link
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() && classify(&entry.file_name().to_string_lossy()).is_some() {
                found.files.push(entry.path());
            }
        }
    }
    found.files.sort();
    found
}

/// How many directories could not be read, in the singular or the plural.
fn unreadable_note(count: usize, root: &Path) -> String {
    if count == 1 {
        format!(
            "1 directory under {} could not be read and is not covered: run as root to see it.",
            root.display()
        )
    } else {
        format!(
            "{count} directories under {} could not be read and are not covered: run as root to see them.",
            root.display()
        )
    }
}

/// The limits of one walk, as sentences; empty when it saw everything.
pub fn scan_notes(root: &Path, scan: &Scan) -> Vec<String> {
    let mut notes = Vec::new();
    if scan.unreadable_dirs > 0 {
        notes.push(unreadable_note(scan.unreadable_dirs, root));
    }
    if scan.truncated {
        notes.push(format!(
            "The walk stopped after {} directories: the rest of {} is not covered.",
            scan.dirs,
            root.display()
        ));
    }
    notes
}

pub fn render(root: &Path, items: &[Pending], notes: &[String]) -> String {
    let mut s = String::new();
    for item in items {
        s.push_str(&format!("{}\n", visible(&item.pending)));
        s.push_str(&format!("    belongs to: {}", visible(&item.live)));
        match (&item.owner, item.live_missing) {
            (_, true) => s.push_str("   (missing)"),
            (Some(owner), false) => s.push_str(&format!("   (package {owner})")),
            (None, false) => {}
        }
        s.push('\n');
        s.push_str(&format!("    {}: {}\n", item.suffix, item.meaning));
        let date = |d: &Option<String>| d.clone().unwrap_or_else(|| "unknown".to_string());
        if item.live_missing {
            s.push_str(&format!("    written: {}\n", date(&item.pending_date)));
        } else {
            s.push_str(&format!(
                "    written: {}; your file: {}\n",
                date(&item.pending_date),
                date(&item.live_date)
            ));
        }
        s.push_str(&format!("    {}\n", item.advice));
        for command in &item.commands {
            s.push_str(&format!("      {command}\n"));
        }
        s.push('\n');
    }
    if items.is_empty() {
        s.push_str(&format!(
            "No unmerged configuration file found under {}.\n",
            root.display()
        ));
    } else {
        s.push_str(&format!(
            "{} pending file(s) under {}. Nothing was changed: `configs` only reports.\n",
            items.len(),
            root.display()
        ));
    }
    for note in notes {
        s.push_str(&format!("{note}\n"));
    }
    s
}

/// `yapacc configs [--json]`: print the report. Reads only.
pub fn run(json: bool) -> Result<()> {
    let root = Path::new("/etc");
    let found = scan(root, MAX_DIRS);
    let backend = crate::native::native_backend().ok();
    let owner = |path: &str| -> Option<String> {
        backend
            .as_ref()
            .and_then(|b| b.owner_of(path).ok().flatten())
    };
    let items: Vec<Pending> = found
        .files
        .iter()
        .filter_map(|p| describe(p, &owner, &crate::util::have))
        .collect();
    let mut notes = scan_notes(root, &found);
    let gentoo: Vec<&str> = ["etc-update", "dispatch-conf"]
        .into_iter()
        .filter(|tool| crate::util::have(tool))
        .collect();
    if !gentoo.is_empty() {
        notes.push(format!(
            "Portage's own pending files (._cfgNNNN_NAME) are not listed here: run `sudo {}` for those.",
            gentoo[0]
        ));
    }
    if json {
        match serde_json::to_string_pretty(&items) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                return Err(crate::Error::from(format!(
                    "json serialization failed: {e}"
                )));
            }
        }
        // the array has no place for them, and they must not be lost
        for note in &notes {
            eprintln!("note: {note}");
        }
    } else {
        print!("{}", render(root, &items, &notes));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unreadable_directories_are_counted_in_the_singular_and_the_plural() {
        let one = unreadable_note(1, Path::new("/etc"));
        assert!(
            one.starts_with("1 directory under /etc could not be read and is not"),
            "{one}"
        );
        let many = unreadable_note(16, Path::new("/etc"));
        assert!(
            many.starts_with("16 directories under /etc could not be read and are not"),
            "{many}"
        );
        assert!(!one.contains("(y/ies)") && !many.contains("(y/ies)"));
    }

    #[test]
    fn hostile_names_are_printable() {
        assert_eq!(super::visible("a\nb\x1b[31m"), "a\\nb\\x1b[31m");
        assert_eq!(super::sh_quote("a\nb"), "$'a\\nb'");
        assert_eq!(super::sh_quote("it's\x1b"), "$'it\\'s\\x1b'");
        assert_eq!(super::sh_quote("sp ace"), "'sp ace'");
        assert_eq!(super::sh_quote("/etc/a.conf"), "/etc/a.conf");
    }

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yapacc-configs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn no_owner(_: &str) -> Option<String> {
        None
    }

    fn no_tool(_: &str) -> bool {
        false
    }

    #[test]
    fn every_suffix_is_classified_with_its_live_name() {
        for (name, live, meaning, family) in [
            ("pacman.conf.pacnew", "pacman.conf", NEW, "pacman"),
            ("mkinitcpio.conf.pacsave", "mkinitcpio.conf", OLD, "pacman"),
            ("sshd_config.dpkg-dist", "sshd_config", NEW, "dpkg"),
            ("sshd_config.dpkg-new", "sshd_config", INTERRUPTED, "dpkg"),
            ("sshd_config.dpkg-old", "sshd_config", OLD, "dpkg"),
            ("grub.ucf-dist", "grub", NEW, "dpkg"),
            ("dnf.conf.rpmnew", "dnf.conf", NEW, "rpm"),
            ("dnf.conf.rpmsave", "dnf.conf", OLD, "rpm"),
            ("inittab.apk-new", "inittab", NEW, "apk"),
        ] {
            let (kind, found) = classify(name).unwrap_or_else(|| panic!("{name} not classified"));
            assert_eq!(
                (found, kind.meaning, kind.family),
                (live, meaning, family),
                "{name}"
            );
        }
        for other in [
            "pacman.conf",
            ".pacnew",
            "notes.pacnew.txt",
            "file.rpmorig",
            "",
        ] {
            assert!(classify(other).is_none(), "{other:?}");
        }
    }

    #[test]
    fn the_merge_tool_is_named_only_when_installed() {
        let pacdiff_only = |tool: &str| tool == "pacdiff";
        assert_eq!(merge_tool("pacman", &pacdiff_only), Some("pacdiff -s"));
        assert_eq!(merge_tool("rpm", &pacdiff_only), None);
        assert_eq!(
            merge_tool("rpm", &|t: &str| t == "rpmconf"),
            Some("sudo rpmconf -a")
        );
        assert_eq!(merge_tool("dpkg", &|_: &str| true), None);
        assert_eq!(merge_tool("pacman", &no_tool), None);
    }

    #[test]
    fn the_walk_finds_pending_files_and_does_not_follow_links() {
        let dir = temp("walk");
        std::fs::create_dir_all(dir.join("ssh/deep")).unwrap();
        std::fs::write(dir.join("pacman.conf"), b"a").unwrap();
        std::fs::write(dir.join("pacman.conf.pacnew"), b"b").unwrap();
        std::fs::write(dir.join("ssh/deep/sshd_config.dpkg-dist"), b"c").unwrap();
        std::fs::write(dir.join("ssh/plain"), b"d").unwrap();
        // a link to a directory, and a link that looks like a pending file
        let _ = std::os::unix::fs::symlink(dir.join("ssh"), dir.join("ssh-link"));
        let _ = std::os::unix::fs::symlink(dir.join("pacman.conf"), dir.join("x.rpmnew"));

        let found = scan(&dir, MAX_DIRS);
        assert_eq!(
            found.files,
            [
                dir.join("pacman.conf.pacnew"),
                dir.join("ssh/deep/sshd_config.dpkg-dist")
            ]
        );
        assert_eq!(
            (found.dirs, found.unreadable_dirs, found.truncated),
            (3, 0, false)
        );
        assert!(scan_notes(&dir, &found).is_empty());

        let capped = scan(&dir, 1);
        assert!(capped.truncated);
        assert_eq!(capped.dirs, 1);
        assert_eq!(scan_notes(&dir, &capped).len(), 1);

        let missing = scan(&dir.join("does-not-exist"), MAX_DIRS);
        assert_eq!(missing.unreadable_dirs, 1);
        assert!(scan_notes(&dir, &missing)[0].contains("run as root"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pending_file_is_described_against_its_live_file() {
        let dir = temp("describe");
        std::fs::write(dir.join("a.conf"), b"mine\n").unwrap();
        std::fs::write(dir.join("a.conf.pacnew"), b"theirs, longer\n").unwrap();
        std::fs::write(dir.join("b.conf"), b"same\n").unwrap();
        std::fs::write(dir.join("b.conf.rpmnew"), b"same\n").unwrap();
        std::fs::write(dir.join("c.conf"), b"abcd\n").unwrap();
        std::fs::write(dir.join("c.conf.dpkg-dist"), b"abce\n").unwrap();
        std::fs::write(dir.join("gone.conf.pacsave"), b"old\n").unwrap();
        let owner = |path: &str| path.ends_with("a.conf").then(|| "apkg".to_string());
        let every_tool = |_: &str| true;

        let a = describe(&dir.join("a.conf.pacnew"), &owner, &every_tool).unwrap();
        assert_eq!(a.live, dir.join("a.conf").to_string_lossy());
        assert_eq!((a.suffix, a.meaning), (".pacnew", NEW));
        assert_eq!((a.identical, a.live_missing), (Some(false), false));
        assert_eq!(a.owner.as_deref(), Some("apkg"));
        assert!(a.advice.starts_with("the files differ"));
        assert_eq!(
            a.commands,
            [
                format!("diff -u {} {}", sh_quote(&a.live), sh_quote(&a.pending)),
                "pacdiff -s".to_string()
            ]
        );
        let date = a.pending_date.clone().unwrap();
        assert_eq!(date.len(), 10, "{date}");
        assert_eq!(date.as_bytes()[4], b'-', "{date}");
        assert!(a.live_date.is_some());

        let b = describe(&dir.join("b.conf.rpmnew"), &no_owner, &every_tool).unwrap();
        assert_eq!(b.identical, Some(true));
        assert_eq!(b.advice, "identical: safe to delete the pending file");
        // nothing to merge: only the comparison is offered
        assert_eq!(b.commands.len(), 1);
        assert_eq!(b.owner, None);

        // same length, different bytes
        let c = describe(&dir.join("c.conf.dpkg-dist"), &no_owner, &no_tool).unwrap();
        assert_eq!(c.identical, Some(false));
        assert_eq!(c.commands.len(), 1);

        let gone = describe(&dir.join("gone.conf.pacsave"), &owner, &every_tool).unwrap();
        assert!(gone.live_missing);
        assert_eq!((gone.identical, gone.live_date.clone()), (None, None));
        assert!(gone.advice.starts_with("the live file is missing"));
        assert!(gone.commands.is_empty());
        // A5: a .pacsave whose live file is gone is not said to have "the
        // package's version live"
        assert_eq!(
            gone.meaning,
            "your old file, saved; the live file no longer exists"
        );
        std::fs::write(dir.join("kept.conf"), b"new\n").unwrap();
        std::fs::write(dir.join("kept.conf.pacsave"), b"old\n").unwrap();
        let kept = describe(&dir.join("kept.conf.pacsave"), &owner, &every_tool).unwrap();
        assert_eq!(kept.meaning, OLD);
        // a new default beside no live file keeps its own meaning
        for k in KINDS {
            assert_eq!(meaning_of(k, false), k.meaning);
            assert_eq!(meaning_of(k, true) == k.meaning, k.meaning != OLD);
        }

        assert!(describe(&dir.join("a.conf"), &no_owner, &no_tool).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_a_shell_would_touch_are_quoted() {
        assert_eq!(sh_quote("/etc/ssh/sshd_config"), "/etc/ssh/sshd_config");
        assert_eq!(sh_quote("/etc/my file"), "'/etc/my file'");
        assert_eq!(sh_quote("/etc/it's"), "'/etc/it'\\''s'");
    }

    fn item() -> Pending {
        Pending {
            pending: "/etc/pacman.conf.pacnew".into(),
            live: "/etc/pacman.conf".into(),
            suffix: ".pacnew",
            meaning: NEW,
            pending_date: Some("2026-10-01".into()),
            live_date: Some("2026-03-02".into()),
            live_missing: false,
            identical: Some(false),
            owner: Some("pacman".into()),
            advice: advice(false, Some(false)),
            commands: vec![
                "diff -u /etc/pacman.conf /etc/pacman.conf.pacnew".into(),
                "pacdiff -s".into(),
            ],
        }
    }

    #[test]
    fn the_report_names_the_commands_and_says_nothing_changed() {
        let etc = Path::new("/etc");
        let out = render(
            etc,
            &[item()],
            &["2 directories could not be read.".to_string()],
        );
        let expected = "\
/etc/pacman.conf.pacnew
    belongs to: /etc/pacman.conf   (package pacman)
    .pacnew: the package's new default; your file was kept
    written: 2026-10-01; your file: 2026-03-02
    the files differ: compare them, merge what you want, then delete the pending file
      diff -u /etc/pacman.conf /etc/pacman.conf.pacnew
      pacdiff -s

1 pending file(s) under /etc. Nothing was changed: `configs` only reports.
2 directories could not be read.
";
        assert_eq!(out, expected);
        assert_eq!(
            render(etc, &[], &[]),
            "No unmerged configuration file found under /etc.\n"
        );
    }

    #[test]
    fn json_is_an_array_of_pending_files() {
        let v = serde_json::to_value(vec![item()]).unwrap();
        assert_eq!(v[0]["pending"], "/etc/pacman.conf.pacnew");
        assert_eq!(v[0]["live"], "/etc/pacman.conf");
        assert_eq!(v[0]["identical"], false);
        assert_eq!(v[0]["live_missing"], false);
        assert_eq!(v[0]["owner"], "pacman");
        assert_eq!(
            v[0]["commands"][0],
            "diff -u /etc/pacman.conf /etc/pacman.conf.pacnew"
        );
    }
}
