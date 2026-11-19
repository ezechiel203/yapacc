// yapacc — version comparison, ported from libalpm and rpm.
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
// This file is a Rust port of `lib/libalpm/version.c` of pacman:
//
//   Copyright (c) 2006-2025 Pacman Development Team
//
//   This program is free software; you can redistribute it and/or modify
//   it under the terms of the GNU General Public License as published by
//   the Free Software Foundation; either version 2 of the License, or
//   (at your option) any later version.
//
// In the words of that file, some of its functions "have been adopted from
// the rpm source, notably 'rpmvercmp' located at lib/rpmvercmp.c and
// 'parseEVR' located at lib/rpmds.c" (rpm is distributed under the GNU
// General Public License, version 2 or later). The comparison of rpm
// versions below follows the current `rpmio/rpmvercmp.c` and `rpmio/rpmver.c`
// of rpm, which add the `~` and `^` separators. The port is distributed,
// with the rest of yapacc, under version 2 of that licence only, which the
// "or any later version" wording of the originals permits.
//
//! Version comparison as the package managers do it. Three orders, because
//! they differ:
//!
//! - [`vercmp_full`]: pacman's (`alpm_pkg_vercmp`, the `vercmp` tool). A
//!   release is compared only when both versions have one; a trailing
//!   alphabetic segment is older (`1.0rc < 1.0`); `~` is a separator like
//!   any other.
//! - [`rpm_evr_cmp`]: rpm's (`rpmverCmp`), for dnf and zypper. `~` sorts
//!   before everything (`1.0~rc1 < 1.0`), `^` after the base version
//!   (`1.0^git1 > 1.0`); a version without a release is older than the
//!   same version with one; whatever is left over wins (`1.0a > 1.0`).
//! - [`deb_vercmp`]: dpkg's (`dpkg --compare-versions`), written from the
//!   algorithm of Debian Policy 5.6.12. `~` sorts before everything, the
//!   end included; letters sort before every other character; a missing
//!   revision is the revision `0`.
//!
//! Pure functions. The tests hold the answers of the real tools.

use std::cmp::Ordering;

/// Compare two pacman versions `[epoch:]pkgver[-pkgrel]` as `vercmp` does.
pub fn vercmp_full(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (ea, va, ra) = alpm_parse_evr(a);
    let (eb, vb, rb) = alpm_parse_evr(b);
    alpm_segments(ea, eb)
        .then_with(|| alpm_segments(va, vb))
        .then_with(|| match (ra, rb) {
            // the release counts only when both versions have one
            (Some(ra), Some(rb)) => alpm_segments(ra, rb),
            _ => Ordering::Equal,
        })
}

/// libalpm's `parseEVR`: the epoch is the digits before a `:` (`0` when
/// there is none, and when it is empty); the release is what follows the
/// last `-`.
fn alpm_parse_evr(evr: &str) -> (&str, &str, Option<&str>) {
    let digits = evr.bytes().take_while(u8::is_ascii_digit).count();
    let (epoch, rest) = if evr.as_bytes().get(digits) == Some(&b':') {
        let epoch = &evr[..digits];
        (
            if epoch.is_empty() { "0" } else { epoch },
            &evr[digits + 1..],
        )
    } else {
        ("0", evr)
    };
    match rest.rfind('-') {
        Some(at) => (epoch, &rest[..at], Some(&rest[at + 1..])),
        None => (epoch, rest, None),
    }
}

/// The end of the leading run of bytes that `keep` accepts.
fn run_end(s: &[u8], keep: impl Fn(&u8) -> bool) -> usize {
    s.iter().position(|c| !keep(c)).unwrap_or(s.len())
}

/// One numeric or alphabetic segment against another of the same kind:
/// numbers by value (leading zeros dropped, the longer one is larger),
/// letters as bytes.
fn segment_cmp(one: &[u8], two: &[u8], numeric: bool) -> Ordering {
    if !numeric {
        return one.cmp(two);
    }
    let strip = |s: &'_ [u8]| -> usize { run_end(s, |c| *c == b'0') };
    let (one, two) = (&one[strip(one)..], &two[strip(two)..]);
    one.len().cmp(&two.len()).then_with(|| one.cmp(two))
}

/// libalpm's `rpmvercmp`: segment by segment.
fn alpm_segments(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (mut one, mut two) = (a.as_bytes(), b.as_bytes());
    while !one.is_empty() && !two.is_empty() {
        let sep1 = run_end(one, |c| !c.is_ascii_alphanumeric());
        let sep2 = run_end(two, |c| !c.is_ascii_alphanumeric());
        // the end of either: finished with the loop
        if sep1 == one.len() || sep2 == two.len() {
            one = &one[sep1..];
            two = &two[sep2..];
            break;
        }
        // separators of different lengths decide
        if sep1 != sep2 {
            return sep1.cmp(&sep2);
        }
        one = &one[sep1..];
        two = &two[sep2..];
        // the first segment that is all digits or all letters, of the
        // kind the first string starts with
        let numeric = one[0].is_ascii_digit();
        let (end1, end2) = if numeric {
            (
                run_end(one, u8::is_ascii_digit),
                run_end(two, u8::is_ascii_digit),
            )
        } else {
            (
                run_end(one, u8::is_ascii_alphabetic),
                run_end(two, u8::is_ascii_alphabetic),
            )
        };
        // different kinds: a numeric segment is newer than an alphabetic one
        if end2 == 0 {
            return if numeric {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        match segment_cmp(&one[..end1], &two[..end2], numeric) {
            Ordering::Equal => {}
            decided => return decided,
        }
        one = &one[end1..];
        two = &two[end2..];
    }
    // all segments compared equal, only the separators differed
    if one.is_empty() && two.is_empty() {
        return Ordering::Equal;
    }
    // a remaining alphabetic string never beats an empty one
    let alpha = |s: &[u8]| s.first().is_some_and(u8::is_ascii_alphabetic);
    if (one.is_empty() && !alpha(two)) || alpha(one) {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// Compare two rpm versions `[epoch:]version[-release]` as rpm does
/// (`rpmverCmp`): for dnf's and zypper's versions.
pub fn rpm_evr_cmp(a: &str, b: &str) -> Ordering {
    let (ea, va, ra) = rpm_parse_evr(a);
    let (eb, vb, rb) = rpm_parse_evr(b);
    ea.cmp(&eb)
        .then_with(|| rpm_segments(va, vb))
        .then_with(|| match (ra, rb) {
            (Some(ra), Some(rb)) => rpm_segments(ra, rb),
            // a version without a release is older than one with
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
        })
}

/// rpm's `rpmverParse`: a missing epoch is 0; the release follows the last
/// `-`.
fn rpm_parse_evr(evr: &str) -> (u64, &str, Option<&str>) {
    let digits = evr.bytes().take_while(u8::is_ascii_digit).count();
    let (epoch, rest) = if evr.as_bytes().get(digits) == Some(&b':') {
        (evr[..digits].parse().unwrap_or(0), &evr[digits + 1..])
    } else {
        (0, evr)
    };
    match rest.rfind('-') {
        Some(at) => (epoch, &rest[..at], Some(&rest[at + 1..])),
        None => (epoch, rest, None),
    }
}

/// rpm's `rpmvercmp`, with its `~` and `^` separators.
fn rpm_segments(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (mut one, mut two) = (a.as_bytes(), b.as_bytes());
    let separator = |c: &u8| !c.is_ascii_alphanumeric() && *c != b'~' && *c != b'^';
    while !one.is_empty() || !two.is_empty() {
        one = &one[run_end(one, separator)..];
        two = &two[run_end(two, separator)..];
        let (c1, c2) = (one.first().copied(), two.first().copied());
        // the tilde sorts before everything else, the end included
        if c1 == Some(b'~') || c2 == Some(b'~') {
            if c1 != Some(b'~') {
                return Ordering::Greater;
            }
            if c2 != Some(b'~') {
                return Ordering::Less;
            }
            one = &one[1..];
            two = &two[1..];
            continue;
        }
        // the caret is like the tilde, except that the base version, the
        // string that ends here, is the older one
        if c1 == Some(b'^') || c2 == Some(b'^') {
            if c1.is_none() {
                return Ordering::Less;
            }
            if c2.is_none() {
                return Ordering::Greater;
            }
            if c1 != Some(b'^') {
                return Ordering::Greater;
            }
            if c2 != Some(b'^') {
                return Ordering::Less;
            }
            one = &one[1..];
            two = &two[1..];
            continue;
        }
        if one.is_empty() || two.is_empty() {
            break;
        }
        let numeric = one[0].is_ascii_digit();
        let (end1, end2) = if numeric {
            (
                run_end(one, u8::is_ascii_digit),
                run_end(two, u8::is_ascii_digit),
            )
        } else {
            (
                run_end(one, u8::is_ascii_alphabetic),
                run_end(two, u8::is_ascii_alphabetic),
            )
        };
        if end2 == 0 {
            return if numeric {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        match segment_cmp(&one[..end1], &two[..end2], numeric) {
            Ordering::Equal => {}
            decided => return decided,
        }
        one = &one[end1..];
        two = &two[end2..];
    }
    // whichever version still has characters left over wins
    one.len().min(1).cmp(&two.len().min(1))
}

/// Compare two Debian versions `[epoch:]upstream[-revision]` as dpkg does
/// (`dpkg --compare-versions`): for apt's versions.
pub fn deb_vercmp(a: &str, b: &str) -> Ordering {
    let (ea, ua, ra) = deb_parse(a);
    let (eb, ub, rb) = deb_parse(b);
    ea.cmp(&eb)
        .then_with(|| deb_part_cmp(ua, ub))
        .then_with(|| deb_part_cmp(ra, rb))
}

/// Epoch (0 when there is none), upstream version and revision: the
/// revision follows the last `-`, and is empty when there is none (an empty
/// revision compares equal to `0`).
fn deb_parse(version: &str) -> (u64, &str, &str) {
    let digits = version.bytes().take_while(u8::is_ascii_digit).count();
    let (epoch, rest) = if version.as_bytes().get(digits) == Some(&b':') {
        (
            version[..digits].parse().unwrap_or(0),
            &version[digits + 1..],
        )
    } else {
        (0, version)
    };
    match rest.rfind('-') {
        Some(at) => (epoch, &rest[..at], &rest[at + 1..]),
        None => (epoch, rest, ""),
    }
}

/// The weight of one character outside a number: `~` before the end of the
/// string, the end (and a digit, which ends the non-digit part) before
/// letters, letters before everything else.
fn deb_order(c: Option<u8>) -> i32 {
    match c {
        Some(b'~') => -1,
        None => 0,
        Some(c) if c.is_ascii_digit() => 0,
        Some(c) if c.is_ascii_alphabetic() => i32::from(c),
        Some(c) => i32::from(c) + 256,
    }
}

/// One part (upstream or revision) against another: alternately a run of
/// non-digits, compared character by character with [`deb_order`], and a
/// run of digits, compared as a number.
fn deb_part_cmp(a: &str, b: &str) -> Ordering {
    let (mut one, mut two) = (a.as_bytes(), b.as_bytes());
    while !one.is_empty() || !two.is_empty() {
        loop {
            let c1 = one.first().copied().filter(|c| !c.is_ascii_digit());
            let c2 = two.first().copied().filter(|c| !c.is_ascii_digit());
            if c1.is_none() && c2.is_none() {
                break;
            }
            match deb_order(c1).cmp(&deb_order(c2)) {
                Ordering::Equal => {}
                decided => return decided,
            }
            one = &one[usize::from(c1.is_some())..];
            two = &two[usize::from(c2.is_some())..];
        }
        let end1 = run_end(one, u8::is_ascii_digit);
        let end2 = run_end(two, u8::is_ascii_digit);
        match segment_cmp(&one[..end1], &two[..end2], true) {
            Ordering::Equal => {}
            decided => return decided,
        }
        one = &one[end1..];
        two = &two[end2..];
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(o: Ordering) -> i32 {
        match o {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    /// The answers of pacman's own `vercmp` (pacman 7.1.0, 2026-11-19) for
    /// the cases of its test suite (test/util/vercmptest.sh) and for
    /// versions seen in the wild.
    const VERCMP: &[(&str, &str, i32)] = &[
        ("1.5.0", "1.5.0", 0),
        ("1.5.1", "1.5.0", 1),
        ("1.5.1", "1.5", 1),
        ("1.5.0-1", "1.5.0-1", 0),
        ("1.5.0-1", "1.5.0-2", -1),
        ("1.5.0-1", "1.5.1-1", -1),
        ("1.5.0-2", "1.5.1-1", -1),
        ("1.5-1", "1.5.1-1", -1),
        ("1.5-2", "1.5.1-1", -1),
        ("1.5-2", "1.5.1-2", -1),
        ("1.5", "1.5-1", 0),
        ("1.5-1", "1.5", 0),
        ("1.1-1", "1.1", 0),
        ("1.0-1", "1.1", -1),
        ("1.1-1", "1.0", 1),
        ("1.5b-1", "1.5-1", -1),
        ("1.5b", "1.5", -1),
        ("1.5b-1", "1.5", -1),
        ("1.5b", "1.5.1", -1),
        ("1.0a", "1.0alpha", -1),
        ("1.0alpha", "1.0b", -1),
        ("1.0b", "1.0beta", -1),
        ("1.0beta", "1.0rc", -1),
        ("1.0rc", "1.0", -1),
        ("1.5.a", "1.5", 1),
        ("1.5.b", "1.5.a", 1),
        ("1.5.1", "1.5.b", 1),
        ("1.5.b-1", "1.5.b", 0),
        ("1.5-1", "1.5.b", -1),
        ("2.0", "2_0", 0),
        ("2.0_a", "2_0.a", 0),
        ("2.0a", "2.0.a", -1),
        ("2___a", "2_a", 1),
        ("0:1.0", "0:1.0", 0),
        ("0:1.0", "0:1.1", -1),
        ("1:1.0", "0:1.0", 1),
        ("1:1.0", "0:1.1", 1),
        ("1:1.0", "2:1.1", -1),
        ("1:1.0", "0:1.0-1", 1),
        ("1:1.0-1", "0:1.1-1", 1),
        ("0:1.0", "1.0", 0),
        ("0:1.0", "1.1", -1),
        ("0:1.1", "1.0", 1),
        ("1:1.0", "1.0", 1),
        ("1:1.0", "1.1", 1),
        ("1:1.1", "1.1", 1),
        ("1.1", "1.a", 1),
        ("1.a", "1.1", -1),
        ("2a", "10", -1),
        ("1.2a", "1.10", -1),
        ("1.0~rc1", "1.0", 1),
        ("1.0~rc1", "1.0~rc2", -1),
        ("1.01", "1.1", 0),
        ("1.9-1", "1.10-1", -1),
        ("1.0-2", "1.0-10", -1),
        ("1.0", "1.0.1", -1),
        ("9.12-2", "9.12-3", -1),
        ("6.12.57.arch1-1", "6.12.58.arch1-1", -1),
        ("1:2.7.3-1", "1:2.7.4-1", -1),
        ("r123.abc", "r124.def", -1),
        ("1.0.r5.gabc", "1.0.1", -1),
        ("20261111", "20261117", -1),
        ("1.0+git20260101", "1.0", 1),
        ("1.0", "1.0+git20260101", -1),
        (":1.0", "1.0", 0),
        ("1.0-", "1.0", 0),
    ];

    #[test]
    fn pacman_versions_compare_as_vercmp_does() {
        for (a, b, want) in VERCMP {
            assert_eq!(sign(vercmp_full(a, b)), *want, "vercmp {a} {b}");
            // and the other way round
            assert_eq!(sign(vercmp_full(b, a)), -*want, "vercmp {b} {a}");
        }
    }

    /// The answers of rpm itself (`rpm.vercmp` of rpm 6.1.0 on Fedora,
    /// 2026-11-19) for the cases of its test suite (tests/rpmvercmp.at) and
    /// for versions of dnf and zypper systems.
    const RPM: &[(&str, &str, i32)] = &[
        ("1.0", "1.0", 0),
        ("1.0", "2.0", -1),
        ("2.0", "1.0", 1),
        ("2.0.1", "2.0.1", 0),
        ("2.0", "2.0.1", -1),
        ("2.0.1", "2.0", 1),
        ("2.0.1a", "2.0.1a", 0),
        ("2.0.1a", "2.0.1", 1),
        ("2.0.1", "2.0.1a", -1),
        ("5.5p1", "5.5p1", 0),
        ("5.5p1", "5.5p2", -1),
        ("5.5p2", "5.5p1", 1),
        ("5.5p10", "5.5p10", 0),
        ("5.5p1", "5.5p10", -1),
        ("5.5p10", "5.5p1", 1),
        ("10xyz", "10.1xyz", -1),
        ("10.1xyz", "10xyz", 1),
        ("xyz10", "xyz10", 0),
        ("xyz10", "xyz10.1", -1),
        ("xyz10.1", "xyz10", 1),
        ("xyz.4", "xyz.4", 0),
        ("xyz.4", "8", -1),
        ("8", "xyz.4", 1),
        ("xyz.4", "2", -1),
        ("2", "xyz.4", 1),
        ("5.5p2", "5.6p1", -1),
        ("5.6p1", "5.5p2", 1),
        ("5.6p1", "6.5p1", -1),
        ("6.5p1", "5.6p1", 1),
        ("6.0.rc1", "6.0", 1),
        ("6.0", "6.0.rc1", -1),
        ("10b2", "10a1", 1),
        ("10a2", "10b2", -1),
        ("1.0aa", "1.0aa", 0),
        ("1.0a", "1.0aa", -1),
        ("1.0aa", "1.0a", 1),
        ("10.0001", "10.0001", 0),
        ("10.0001", "10.1", 0),
        ("10.1", "10.0001", 0),
        ("10.0001", "10.0039", -1),
        ("10.0039", "10.0001", 1),
        ("4.999.9", "5.0", -1),
        ("5.0", "4.999.9", 1),
        ("20101121", "20101121", 0),
        ("20101121", "20101122", -1),
        ("20101122", "20101121", 1),
        ("2_0", "2_0", 0),
        ("2.0", "2_0", 0),
        ("2_0", "2.0", 0),
        ("a", "a", 0),
        ("a+", "a+", 0),
        ("a+", "a_", 0),
        ("a_", "a+", 0),
        ("+a", "+a", 0),
        ("+a", "_a", 0),
        ("_a", "+a", 0),
        ("+_", "+_", 0),
        ("_+", "+_", 0),
        ("_+", "_+", 0),
        ("+", "_", 0),
        ("_", "+", 0),
        ("1.0~rc1", "1.0~rc1", 0),
        ("1.0~rc1", "1.0", -1),
        ("1.0", "1.0~rc1", 1),
        ("1.0~rc1", "1.0~rc2", -1),
        ("1.0~rc2", "1.0~rc1", 1),
        ("1.0~rc1~git123", "1.0~rc1~git123", 0),
        ("1.0~rc1~git123", "1.0~rc1", -1),
        ("1.0~rc1", "1.0~rc1~git123", 1),
        ("1.0^", "1.0^", 0),
        ("1.0^", "1.0", 1),
        ("1.0", "1.0^", -1),
        ("1.0^git1", "1.0^git1", 0),
        ("1.0^git1", "1.0", 1),
        ("1.0", "1.0^git1", -1),
        ("1.0^git1", "1.0^git2", -1),
        ("1.0^git2", "1.0^git1", 1),
        ("1.0^git1", "1.01", -1),
        ("1.01", "1.0^git1", 1),
        ("1.0^20160101", "1.0^20160101", 0),
        ("1.0^20160101", "1.0.1", -1),
        ("1.0.1", "1.0^20160101", 1),
        ("1.0^20160101^git1", "1.0^20160101^git1", 0),
        ("1.0^20160102", "1.0^20160101^git1", 1),
        ("1.0^20160101^git1", "1.0^20160102", -1),
        ("1.0~rc1^git1", "1.0~rc1^git1", 0),
        ("1.0~rc1^git1", "1.0~rc1", 1),
        ("1.0~rc1", "1.0~rc1^git1", -1),
        ("1.0^git1~pre", "1.0^git1~pre", 0),
        ("1.0^git1", "1.0^git1~pre", 1),
        ("1.0^git1~pre", "1.0^git1", -1),
        ("1.0-69.1", "1.0-68.1", 1),
        ("2.44-3.1", "2.44-4.1", -1),
        ("1:1.0-1", "2:0.5-1", -1),
        ("2:0.7.2-6.el9_9", "2:0.6.0-4.el9", 1),
        ("7.3.4-300.fc45", "7.3.3-300.fc45", 1),
        ("7.2.9-200.fc45", "7.3.3-300.fc45", -1),
        ("1.24-1.el9", "1.24-2.el9", -1),
        ("5.1.8-6.el9_1", "5.1.8-9.el9", -1),
        ("1.0-1", "1:0.5-1", -1),
        ("0:1.0-1", "1.0-1", 0),
        ("1.0", "1.0-1", -1),
        ("1.0-1", "1.0", 1),
    ];

    #[test]
    fn rpm_versions_compare_as_rpm_does() {
        for (a, b, want) in RPM {
            assert_eq!(sign(rpm_evr_cmp(a, b)), *want, "rpm.vercmp {a} {b}");
            assert_eq!(sign(rpm_evr_cmp(b, a)), -*want, "rpm.vercmp {b} {a}");
        }
    }

    /// The answers of `dpkg --compare-versions A lt|eq|gt B` (dpkg 1.22,
    /// debian:stable, 2026-11-19), for versions seen in dpkg.log and for the
    /// corners of the order.
    const DPKG: &[(&str, &str, i32)] = &[
        ("1.0", "1.0", 0),
        ("1.1.0-2+deb13u1", "1.1.0-2+b7", 1),
        (
            "1:1.3.dfsg+really1.3.1-1+b1",
            "1:1.3.dfsg+really1.3.1-1+deb13u1",
            -1,
        ),
        ("3.5.7-1~deb13u2", "3.5.7-1~deb13u3", -1),
        ("2:9.1.1230-2", "2:9.1.1230-2+deb13u2", -1),
        ("20250419", "20250419+deb13u1", -1),
        ("2026c-0+deb13u1", "2026f-0+deb13u1", -1),
        ("1.0~rc1", "1.0", -1),
        ("8.14.1-2+deb13u7", "8.14.1-2+deb13u5", 1),
        ("2.41-12+deb13u4", "2.41-12", 1),
        ("1.0~rc1-1", "1.0~rc1-1~bpo13+1", 1),
        ("1:0.9", "2.0", 1),
        ("5.38.2-3.2ubuntu0.6", "5.38.2-3.2ubuntu0.7", -1),
        ("1.0-1", "1.0-1+b1", -1),
        ("1.0a", "1.0+", -1),
        ("1.0", "1.0.0", -1),
        ("09", "9", 0),
        ("1.2.3", "1.2.3-0", 0),
        ("2.3.8-2", "2.3.10-1", -1),
        ("1.0-1", "1.0-1.1", -1),
        ("1.0~~", "1.0~", -1),
        ("1.0~", "1.0", -1),
        ("0:1.0", "1.0", 0),
        ("1.0-1-2", "1.0-2-1", -1),
        ("2.2.1-1", "2.2.1-1", 0),
        ("1.0+dfsg", "1.0.dfsg", -1),
        ("1.0z", "1.0+", -1),
        ("a1", "a01", 0),
        ("1.0-a", "1.0-1", 1),
    ];

    #[test]
    fn debian_versions_compare_as_dpkg_does() {
        for (a, b, want) in DPKG {
            assert_eq!(sign(deb_vercmp(a, b)), *want, "dpkg {a} {b}");
            assert_eq!(sign(deb_vercmp(b, a)), -*want, "dpkg {b} {a}");
        }
    }

    #[test]
    fn the_two_orders_differ_where_the_tools_do() {
        use Ordering::*;
        // a pre-release written with a tilde
        assert_eq!(rpm_evr_cmp("1.0~rc1", "1.0"), Less);
        assert_eq!(vercmp_full("1.0~rc1", "1.0"), Greater);
        // a trailing letter
        assert_eq!(rpm_evr_cmp("1.0a", "1.0"), Greater);
        assert_eq!(vercmp_full("1.0a", "1.0"), Less);
        // a release on one side only
        assert_eq!(rpm_evr_cmp("1.0", "1.0-1"), Less);
        assert_eq!(vercmp_full("1.0", "1.0-1"), Equal);
    }
}
