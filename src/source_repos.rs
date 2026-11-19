use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub kind: String, // apt-list | apt-deb822 | rpm-repo | ...
    pub id: String,
    pub uri: String,
    pub suite: String,
    pub components: String,
    pub enabled: bool,
    /// Parsed extra key=value options (consumed by callers/tests).
    #[allow(dead_code)]
    pub options: String,
    pub origin: String, // file path
}

fn read_to_string_lossy(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("failed to read {}: {e}", p.display()))
}

pub fn parse_apt_list_file(path: &Path) -> Result<Vec<SourceEntry>, String> {
    let s = read_to_string_lossy(path)?;
    let mut out = Vec::new();
    for (i, line) in s.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // deb [ options ] uri suite [component1] [component2] [...]
        // deb-src ...
        let mut rest = t.to_string();
        let typ = rest.split_whitespace().next().unwrap_or("").to_string();
        if typ != "deb" && typ != "deb-src" {
            continue;
        }
        rest = rest[typ.len()..].trim().to_string();
        let mut opts = String::new();
        if rest.starts_with('[')
            && let Some(end) = rest.find(']')
        {
            opts = rest[1..end].trim().to_string();
            rest = rest[end + 1..].trim().to_string();
        }
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let uri = parts[0].to_string();
        let suite = parts[1].to_string();
        let components = if parts.len() > 2 {
            parts[2..].join(" ")
        } else {
            String::new()
        };
        out.push(SourceEntry {
            kind: "apt-list".into(),
            id: format!("{}:{}:{}", path.display(), i + 1, typ),
            uri,
            suite,
            components,
            enabled: true,
            options: opts,
            origin: path.display().to_string(),
        });
    }
    Ok(out)
}

pub fn parse_apt_deb822_file(path: &Path) -> Result<Vec<SourceEntry>, String> {
    let s = read_to_string_lossy(path)?;
    let mut out = Vec::new();
    let mut stanza: Vec<(String, String)> = Vec::new();
    let flush = |st: &[(String, String)], path: &Path, idx: usize, out: &mut Vec<SourceEntry>| {
        if st.is_empty() {
            return;
        }
        let get = |k: &str| {
            st.iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let types = get("Types");
        if types.is_empty() {
            return;
        }
        let enabled = !get("Enabled").eq_ignore_ascii_case("no")
            && !get("Enabled").eq_ignore_ascii_case("false");
        let uri = get("URIs").to_string();
        let suite = get("Suites").to_string();
        let components = get("Components").to_string();
        let opts = st
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    k.as_str(),
                    "Types" | "URIs" | "Suites" | "Components" | "Enabled"
                )
            })
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        out.push(SourceEntry {
            kind: "apt-deb822".into(),
            id: format!("{}#{}", path.display(), idx),
            uri,
            suite,
            components,
            enabled,
            options: opts,
            origin: path.display().to_string(),
        });
    };
    let mut idx = 0usize;
    for line in s.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            idx += 1;
            flush(&stanza, path, idx, &mut out);
            stanza.clear();
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            stanza.push((k.trim().to_string(), v.trim().to_string()));
        } else if let Some(last) = stanza.last_mut() {
            // continuation
            last.1.push(' ');
            last.1.push_str(line.trim());
        }
    }
    Ok(out)
}

pub fn parse_apt_sources(root: &Path) -> Result<Vec<SourceEntry>, String> {
    let mut out = Vec::new();
    let list = root.join("sources.list");
    if list.exists() {
        out.extend(parse_apt_list_file(&list)?);
    }
    let d = root.join("sources.list.d");
    if d.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&d)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|x| x.path()))
            .collect();
        entries.sort();
        for p in entries {
            match p.extension().and_then(|s| s.to_str()) {
                Some("list") => out.extend(parse_apt_list_file(&p)?),
                Some("sources") => out.extend(parse_apt_deb822_file(&p)?),
                _ => {}
            }
        }
    }
    Ok(out)
}

pub fn parse_rpm_repo_file(path: &Path) -> Result<Vec<SourceEntry>, String> {
    let s = read_to_string_lossy(path)?;
    let mut out = Vec::new();
    let mut section: Option<String> = None;
    let mut kv: Vec<(String, String)> = Vec::new();
    let flush =
        |sec: &Option<String>, kv: &[(String, String)], path: &Path, out: &mut Vec<SourceEntry>| {
            let Some(id) = sec.clone() else {
                return;
            };
            let get = |k: &str| {
                kv.iter()
                    .find(|(a, _)| a.eq_ignore_ascii_case(k))
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
            };
            let enabled = !matches!(get("enabled"), "0" | "false" | "False" | "no" | "No");
            let uri = if !get("baseurl").is_empty() {
                get("baseurl")
            } else if !get("metalink").is_empty() {
                get("metalink")
            } else {
                get("mirrorlist")
            };
            let opts = kv
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.to_ascii_lowercase().as_str(),
                        "name" | "baseurl" | "mirrorlist" | "metalink" | "enabled"
                    )
                })
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            out.push(SourceEntry {
                kind: "rpm-repo".into(),
                id,
                uri: uri.to_string(),
                suite: String::new(),
                components: get("name").to_string(),
                enabled,
                options: opts,
                origin: path.display().to_string(),
            });
        };
    for line in s.lines().chain(std::iter::once("")) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            flush(&section, &kv, path, &mut out);
            section = Some(t[1..t.len() - 1].trim().to_string());
            kv.clear();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            kv.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    flush(&section, &kv, path, &mut out);
    Ok(out)
}

pub fn parse_rpm_repos_dir(dir: &Path) -> Result<Vec<SourceEntry>, String> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .collect();
    entries.sort();
    for p in entries {
        if p.extension().and_then(|s| s.to_str()) == Some("repo") {
            out.extend(parse_rpm_repo_file(&p)?);
        }
    }
    Ok(out)
}

pub fn format_sources(entries: &[SourceEntry]) -> String {
    if entries.is_empty() {
        return "(no source definitions found)".into();
    }
    let mut s = String::new();
    s.push_str(&format!("Found {} source definition(s)\n", entries.len()));
    s.push_str(&format!(
        "{:<10} {:<28} {:<7} {:<36} {}\n",
        "KIND", "ID", "EN", "URI", "SUITE/COMPONENTS (origin)"
    ));
    s.push_str(&"-".repeat(140));
    s.push('\n');
    for e in entries {
        let en = if e.enabled { "yes" } else { "no" };
        let tail = if e.suite.is_empty() && e.components.is_empty() {
            format!("({})", e.origin)
        } else if e.components.is_empty() {
            format!("{} ({})", e.suite, e.origin)
        } else {
            format!("{} {} ({})", e.suite, e.components, e.origin)
        };
        // cut by characters: a byte index can fall inside one
        let cut = |text: &str, max: usize| {
            if text.chars().count() > max {
                format!("{}…", text.chars().take(max - 1).collect::<String>())
            } else {
                text.to_string()
            }
        };
        let id = cut(&e.id, 28);
        let uri = cut(&e.uri, 36);
        s.push_str(&format!(
            "{:<10} {:<28} {:<7} {:<36} {}\n",
            e.kind, id, en, uri, tail
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-srctest-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    /// `yapacc sources rpm` panicked ("end byte index 27 is not a char
    /// boundary") on a repository id or URL with a non-ASCII character at
    /// the place where the column is cut.
    #[test]
    fn a_long_non_ascii_id_or_uri_is_cut_between_characters() {
        let p = write_tmp(
            "accents.repo",
            "[abcdefghijklmnopqrstuvwxyzé-dépôt]\nname=Internal\nbaseurl=https://example.invalid/0123456789é/répertoire/\nenabled=1\n",
        );
        let entries = parse_rpm_repo_file(&p).unwrap();
        let text = format_sources(&entries);
        assert!(text.contains("abcdefghijklmnopqrstuvwxyzé… "), "{text}");
        assert!(
            text.contains("https://example.invalid/0123456789é… "),
            "{text}"
        );
    }

    #[test]
    fn apt_list_parsing() {
        let p = write_tmp(
            "sources.list",
            "# comment\ndeb http://deb.debian.org/debian bookworm main contrib non-free\ndeb [arch=amd64 trusted=yes] http://security.debian.org/debian-security bookworm-security main\ndeb-src http://example.com/debian stable main\n#deb http://disabled.example.com/debian stable main\n",
        );
        let entries = parse_apt_list_file(&p).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].uri, "http://deb.debian.org/debian");
        assert_eq!(entries[0].suite, "bookworm");
        assert_eq!(entries[0].components, "main contrib non-free");
        assert!(entries[0].enabled);
        assert_eq!(entries[1].options, "arch=amd64 trusted=yes");
        assert_eq!(entries[1].suite, "bookworm-security");
    }

    #[test]
    fn apt_deb822_parsing() {
        let p = write_tmp(
            "test.sources",
            "Types: deb\nURIs: http://deb.debian.org/debian\nSuites: bookworm bookworm-updates\nComponents: main contrib\nSigned-By: /usr/share/keyrings/debian.gpg\n\nTypes: deb\nURIs: http://disabled.example.com\nSuites: stable\nEnabled: no\n",
        );
        let entries = parse_apt_deb822_file(&p).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].suite, "bookworm bookworm-updates");
        assert!(entries[0].enabled);
        assert!(
            entries[0]
                .options
                .contains("Signed-By=/usr/share/keyrings/debian.gpg")
        );
        assert!(!entries[1].enabled);
    }

    #[test]
    fn rpm_repo_parsing() {
        let p = write_tmp(
            "test.repo",
            "[fedora]\nname=Fedora $releasever - $basearch\nbaseurl=https://download.example/pub/fedora/linux/releases/$releasever/Everything/$basearch/os/\nenabled=1\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora\n\n[disabled-repo]\nname=Disabled\nbaseurl=https://example.com/repo\nenabled=0\n",
        );
        let entries = parse_rpm_repo_file(&p).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "fedora");
        assert!(entries[0].uri.starts_with("https://download.example"));
        assert!(entries[0].enabled);
        assert!(entries[0].options.contains("gpgcheck=1"));
        assert!(!entries[1].enabled);
    }

    #[test]
    fn apt_sources_root_combines_list_and_deb822() {
        let d = std::env::temp_dir().join(format!("pc-srcroot-{}", std::process::id()));
        let listd = d.join("sources.list.d");
        std::fs::create_dir_all(&listd).unwrap();
        std::fs::write(
            d.join("sources.list"),
            "deb http://a.example/debian stable main\n",
        )
        .unwrap();
        std::fs::write(
            listd.join("x.list"),
            "deb http://b.example/debian stable main\n",
        )
        .unwrap();
        std::fs::write(
            listd.join("y.sources"),
            "Types: deb\nURIs: http://c.example/debian\nSuites: stable\nComponents: main\n",
        )
        .unwrap();
        std::fs::write(listd.join("ignored.txt"), "not a source file\n").unwrap();
        let entries = parse_apt_sources(&d).unwrap();
        assert_eq!(entries.len(), 3);
        let uris: Vec<&str> = entries.iter().map(|e| e.uri.as_str()).collect();
        assert!(uris.contains(&"http://a.example/debian"));
        assert!(uris.contains(&"http://b.example/debian"));
        assert!(uris.contains(&"http://c.example/debian"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
