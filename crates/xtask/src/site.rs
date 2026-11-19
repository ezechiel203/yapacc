// yapacc — website generator.
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! `cargo xtask site` builds the project website into `target/site/`.
//!
//! Sources:
//! * `docs/handbook/SUMMARY.md` — the table of contents (`## Section`
//!   headings and `- [Title](file.md)` items);
//! * `docs/handbook/*.md` — the handbook pages, plain Markdown that also
//!   reads well on a code host;
//! * `site/templates/*.html` — the landing page and the page layout
//!   (`{{name}}` is a variable, `{{> name}}` includes `_name.html`,
//!   `{{demo name}}` includes the terminal capture `site/demos/name.txt`);
//! * `site/assets/` — stylesheet, script, fonts, images; copied as they are.
//!
//! The output is static files with relative links only, so it can be served
//! from any path. Every internal link and anchor is checked; a broken one
//! fails the build.

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

const REPO: &str = "https://github.com/ezechiel203/yapacc";
/// The path the site is served under: the path of the `homepage` URL in
/// Cargo.toml. Only the not-found page needs it. The server answers with
/// that page at any depth, so its links cannot be relative.
const SITE_BASE: &str = "/yapacc/";
const HANDBOOK: &str = "docs/handbook";

struct PageRef {
    title: String,
    file: String,
    section: String,
}

struct Rendered {
    title: String,
    html: String,
    /// (level, id, text) of the h2 and h3 headings.
    toc: Vec<(usize, String, String)>,
    /// (heading id, heading text, plain text below it) for the search index.
    sections: Vec<(String, String, String)>,
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn json_str(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `README.md` is the handbook's front page; every other file keeps its name.
fn out_name(file: &str) -> String {
    let stem = file.trim_end_matches(".md");
    if stem.eq_ignore_ascii_case("README") {
        "index.html".to_string()
    } else {
        format!("{stem}.html")
    }
}

fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn parse_summary(text: &str) -> Result<Vec<PageRef>, String> {
    let mut pages = vec![];
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("## ") {
            section = title.trim().to_string();
        } else if let Some(item) = line.strip_prefix("- [") {
            let (title, rest) = item
                .split_once("](")
                .ok_or_else(|| format!("SUMMARY.md: malformed item `{line}`"))?;
            let file = rest
                .split_once(')')
                .ok_or_else(|| format!("SUMMARY.md: malformed item `{line}`"))?
                .0;
            pages.push(PageRef {
                title: title.to_string(),
                file: file.to_string(),
                section: section.clone(),
            });
        }
    }
    if pages.is_empty() {
        return Err("SUMMARY.md lists no page".to_string());
    }
    Ok(pages)
}

/// Resolve `rel` against `base_dir` (both repository-relative), dropping `.`
/// and `..`.
fn normalize(base_dir: &str, rel: &str) -> String {
    let mut parts: Vec<String> = vec![];
    for comp in Path::new(base_dir).join(rel).components() {
        match comp {
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(p) => parts.push(p.to_string_lossy().to_string()),
            _ => {}
        }
    }
    parts.join("/")
}

/// Handbook pages link to each other as `file.md`; anything else in the
/// repository becomes a link to the code host.
fn rewrite_link(dest: &str, known: &BTreeSet<String>) -> String {
    if dest.starts_with('#') || dest.contains("://") || dest.starts_with("mailto:") {
        return dest.to_string();
    }
    let (path, frag) = match dest.split_once('#') {
        Some((p, f)) => (p, format!("#{f}")),
        None => (dest, String::new()),
    };
    if known.contains(path) {
        return format!("{}{frag}", out_name(path));
    }
    format!("{REPO}/blob/main/{}{frag}", normalize(HANDBOOK, path))
}

fn plain(events: &[Event]) -> String {
    let mut out = String::new();
    for e in events {
        match e {
            Event::Text(t) | Event::Code(t) => out.push_str(t),
            _ => {}
        }
    }
    out
}

const STATUS_WORDS: &[(&str, &str)] = &[
    ("ok", "ok"),
    ("warn", "warn"),
    ("fail", "fail"),
    ("n/a", "na"),
];

/// A terminal capture: `$ ` and `# ` lines are commands, the rest is output.
pub fn console_html(text: &str) -> String {
    let mut out = String::new();
    for line in text.trim_end().lines() {
        let prompt = if line.starts_with("$ ") {
            Some("$")
        } else if line.starts_with("# ") {
            Some("#")
        } else {
            None
        };
        if let Some(p) = prompt {
            let cmd = &line[2..];
            let (cmd, comment) = match cmd.find("  # ") {
                Some(i) => (&cmd[..i], &cmd[i..]),
                None => (cmd, ""),
            };
            out.push_str(&format!(
                "<span class=\"ln cmd\"><span class=\"p\">{p}</span> <span class=\"c\">{}</span>",
                esc(cmd)
            ));
            if !comment.is_empty() {
                out.push_str(&format!("<span class=\"k\">{}</span>", esc(comment)));
            }
            out.push_str("</span>\n");
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        let status = STATUS_WORDS
            .iter()
            .find(|(w, _)| line.starts_with(w) && first == *w);
        let body = if let Some((word, class)) = status {
            format!(
                "<span class=\"st-{class}\">{}</span>{}",
                esc(word),
                esc(&line[word.len()..])
            )
        } else if line.starts_with("note:") || line.starts_with("DRY-RUN") {
            format!("<span class=\"k\">{}</span>", esc(line))
        } else {
            esc(line)
        };
        out.push_str(&format!("<span class=\"ln\">{body}</span>\n"));
    }
    out
}

fn code_html(lang: &str, text: &str) -> String {
    let lang = lang.split_whitespace().next().unwrap_or("");
    let inner = match lang {
        "console" => console_html(text),
        "toml" | "sh" | "bash" | "ini" | "conf" => {
            let mut out = String::new();
            for line in text.trim_end().lines() {
                if line.trim_start().starts_with('#') {
                    out.push_str(&format!("<span class=\"k\">{}</span>\n", esc(line)));
                } else {
                    out.push_str(&esc(line));
                    out.push('\n');
                }
            }
            out
        }
        _ => format!("{}\n", esc(text.trim_end())),
    };
    let label = if lang.is_empty() { "text" } else { lang };
    format!(
        "<figure class=\"code\" data-lang=\"{}\"><pre><code>{inner}</code></pre></figure>\n",
        esc(label)
    )
}

fn render(src: &str, known: &BTreeSet<String>) -> Rendered {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_GFM
        | Options::ENABLE_FOOTNOTES;
    let mut iter = Parser::new_ext(src, opts);
    let mut events: Vec<Event> = vec![];
    let mut title: Option<String> = None;
    let mut toc = vec![];
    let mut sections: Vec<(String, String, String)> =
        vec![(String::new(), String::new(), String::new())];
    let mut used: HashMap<String, usize> = HashMap::new();

    while let Some(ev) = iter.next() {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                let mut inner = vec![];
                for e in iter.by_ref() {
                    if matches!(e, Event::End(TagEnd::Heading(_))) {
                        break;
                    }
                    inner.push(e);
                }
                let text = plain(&inner);
                if level == HeadingLevel::H1 && title.is_none() {
                    title = Some(text);
                    continue;
                }
                let base = slugify(&text);
                let n = used.entry(base.clone()).or_insert(0);
                *n += 1;
                let id = if *n == 1 { base } else { format!("{base}-{n}") };
                let mut inner_html = String::new();
                html::push_html(&mut inner_html, inner.into_iter());
                let lv = level as usize;
                events.push(Event::Html(CowStr::from(format!(
                    "<h{lv} id=\"{id}\">{inner_html}<a class=\"anchor\" href=\"#{id}\" aria-label=\"Link to this section\">§</a></h{lv}>\n"
                ))));
                if lv == 2 || lv == 3 {
                    toc.push((lv, id.clone(), text.clone()));
                }
                sections.push((id, text, String::new()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match &kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut text = String::new();
                for e in iter.by_ref() {
                    match e {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(t) => text.push_str(&t),
                        _ => {}
                    }
                }
                events.push(Event::Html(CowStr::from(code_html(&lang, &text))));
            }
            Event::Start(Tag::BlockQuote(Some(kind))) => {
                let (class, label) = match kind {
                    BlockQuoteKind::Note => ("note", "Note"),
                    BlockQuoteKind::Tip => ("tip", "Tip"),
                    BlockQuoteKind::Important => ("important", "Important"),
                    BlockQuoteKind::Warning => ("warning", "Warning"),
                    BlockQuoteKind::Caution => ("caution", "Caution"),
                };
                events.push(Event::Html(CowStr::from(format!(
                    "<aside class=\"callout callout-{class}\"><p class=\"callout-title\">{label}</p>\n"
                ))));
            }
            Event::End(TagEnd::BlockQuote(Some(_))) => {
                events.push(Event::Html(CowStr::from("</aside>\n")));
            }
            Event::Start(Tag::Table(aligns)) => {
                events.push(Event::Html(CowStr::from("<div class=\"table-wrap\">")));
                events.push(Event::Start(Tag::Table(aligns)));
            }
            Event::End(TagEnd::Table) => {
                events.push(Event::End(TagEnd::Table));
                events.push(Event::Html(CowStr::from("</div>\n")));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                events.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: CowStr::from(rewrite_link(&dest_url, known)),
                    title,
                    id,
                }));
            }
            Event::Text(t) => {
                if let Some(last) = sections.last_mut() {
                    last.2.push_str(&t);
                    last.2.push(' ');
                }
                events.push(Event::Text(t));
            }
            Event::Code(t) => {
                if let Some(last) = sections.last_mut() {
                    last.2.push_str(&t);
                    last.2.push(' ');
                }
                events.push(Event::Code(t));
            }
            other => events.push(other),
        }
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    Rendered {
        title: title.unwrap_or_default(),
        html: out,
        toc,
        sections,
    }
}

struct Templates {
    dir: PathBuf,
    demos: PathBuf,
}

impl Templates {
    /// Expand `{{> partial}}` and `{{demo name}}`, then the variables. A
    /// placeholder left over is an error: it is a typo in a template.
    fn fill(&self, name: &str, vars: &BTreeMap<&str, String>) -> Result<String, String> {
        let mut text = read(&self.dir.join(name))?;
        for _ in 0..4 {
            let Some(start) = text.find("{{> ") else {
                break;
            };
            let end = text[start..]
                .find("}}")
                .ok_or_else(|| format!("{name}: unclosed include"))?
                + start;
            let partial = text[start + 4..end].trim().to_string();
            let body = read(&self.dir.join(format!("_{partial}.html")))?;
            text.replace_range(start..end + 2, &body);
        }
        while let Some(start) = text.find("{{demo ") {
            let end = text[start..]
                .find("}}")
                .ok_or_else(|| format!("{name}: unclosed demo"))?
                + start;
            let demo = text[start + 7..end].trim().to_string();
            let body = console_html(&read(&self.demos.join(format!("{demo}.txt")))?);
            text.replace_range(start..end + 2, &body);
        }
        for (key, value) in vars {
            text = text.replace(&format!("{{{{{key}}}}}"), value);
        }
        if let Some(i) = text.find("{{") {
            let rest: String = text[i..].chars().take(40).collect();
            return Err(format!("{name}: unknown placeholder near `{rest}`"));
        }
        Ok(text)
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|e| format!("cannot list {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("cannot copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn workspace_version(root: &Path) -> Result<String, String> {
    let manifest = read(&root.join("Cargo.toml"))?;
    let package = manifest
        .split("[package]")
        .nth(1)
        .ok_or("Cargo.toml has no [package]")?;
    for line in package.lines() {
        if let Some(rest) = line.trim().strip_prefix("version") {
            let v = rest.trim().trim_start_matches('=').trim().trim_matches('"');
            return Ok(v.to_string());
        }
    }
    Err("Cargo.toml: no version in [package]".to_string())
}

fn html_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            html_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "html") {
            out.push(path);
        }
    }
    Ok(())
}

fn attr_values(html: &str, attr: &str) -> Vec<String> {
    let needle = format!(" {attr}=\"");
    let mut out = vec![];
    let mut rest = html;
    while let Some(i) = rest.find(&needle) {
        rest = &rest[i + needle.len()..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    out
}

/// Every `href` and `src` that stays inside the site must point at one of
/// its files, and every fragment at an `id` of its target.
fn check_links(out: &Path) -> Result<usize, String> {
    let mut files = vec![];
    html_files(out, &mut files)?;
    let mut ids: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    let mut bodies: HashMap<PathBuf, String> = HashMap::new();
    for f in &files {
        let body = read(f)?;
        ids.insert(f.clone(), attr_values(&body, "id").into_iter().collect());
        bodies.insert(f.clone(), body);
    }
    let mut broken = vec![];
    let mut checked = 0;
    for f in &files {
        let dir = f.parent().unwrap_or(out);
        let body = &bodies[f];
        let mut links = attr_values(body, "href");
        links.extend(attr_values(body, "src"));
        for link in links {
            if link.contains("://") || link.starts_with("mailto:") || link.starts_with("data:") {
                continue;
            }
            checked += 1;
            let (path, frag) = match link.split_once('#') {
                Some((p, fr)) => (p, Some(fr)),
                None => (link.as_str(), None),
            };
            // A link under SITE_BASE is resolved from the top of the site.
            let (from, path, absolute) = match path.strip_prefix(SITE_BASE) {
                Some(rest) => (out, rest, true),
                None => (dir, path, false),
            };
            let target = if path.is_empty() && !absolute {
                f.clone()
            } else {
                let mut t = PathBuf::new();
                for comp in from.join(path).components() {
                    match comp {
                        Component::ParentDir => {
                            t.pop();
                        }
                        Component::CurDir => {}
                        other => t.push(other),
                    }
                }
                if t.is_dir() { t.join("index.html") } else { t }
            };
            let shown = f.strip_prefix(out).unwrap_or(f).display();
            if !target.exists() {
                broken.push(format!("{shown}: `{link}` points at nothing"));
            } else if let Some(frag) = frag.filter(|fr| !fr.is_empty())
                && let Some(set) = ids.get(&target)
                && !set.contains(frag)
            {
                broken.push(format!("{shown}: `{link}` has no such anchor"));
            }
        }
    }
    if broken.is_empty() {
        Ok(checked)
    } else {
        Err(format!("broken links:\n  {}", broken.join("\n  ")))
    }
}

pub fn build(root: &Path, args: &[String]) -> Result<(), String> {
    let mut out = root.join("target").join("site");
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => {
                out = PathBuf::from(it.next().ok_or("--out needs a directory")?);
            }
            other => {
                return Err(format!(
                    "unknown option `{other}` (usage: cargo xtask site [--out DIR])"
                ));
            }
        }
    }
    let handbook = root.join(HANDBOOK);
    let site = root.join("site");
    let templates = Templates {
        dir: site.join("templates"),
        demos: site.join("demos"),
    };
    let version = workspace_version(root)?;
    let pages = parse_summary(&read(&handbook.join("SUMMARY.md"))?)?;
    let known: BTreeSet<String> = pages.iter().map(|p| p.file.clone()).collect();

    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("cannot clear {}: {e}", out.display()))?;
    }
    copy_dir(&site.join("assets"), &out.join("assets"))?;
    write(&out.join(".nojekyll"), "")?;

    let mut base: BTreeMap<&str, String> = BTreeMap::new();
    base.insert("version", version.clone());
    base.insert("repo", REPO.to_string());

    // The landing page and the not-found page.
    let mut vars = base.clone();
    vars.insert("root", String::new());
    write(
        &out.join("index.html"),
        &templates.fill("landing.html", &vars)?,
    )?;
    vars.insert("root", SITE_BASE.to_string());
    write(&out.join("404.html"), &templates.fill("404.html", &vars)?)?;

    // The handbook.
    let mut search = vec![];
    for (i, page) in pages.iter().enumerate() {
        let rendered = render(&read(&handbook.join(&page.file))?, &known);
        let title = if rendered.title.is_empty() {
            page.title.clone()
        } else {
            rendered.title.clone()
        };
        let name = out_name(&page.file);

        let mut sidebar = String::new();
        let mut current_section = None;
        for p in &pages {
            if current_section != Some(&p.section) {
                if current_section.is_some() {
                    sidebar.push_str("</ul>\n");
                }
                sidebar.push_str(&format!(
                    "<p class=\"nav-section\">{}</p>\n<ul>\n",
                    esc(&p.section)
                ));
                current_section = Some(&p.section);
            }
            let current = if p.file == page.file {
                " aria-current=\"page\""
            } else {
                ""
            };
            sidebar.push_str(&format!(
                "<li><a href=\"{}\"{current}>{}</a></li>\n",
                out_name(&p.file),
                esc(&p.title)
            ));
        }
        sidebar.push_str("</ul>\n");

        let mut toc = String::new();
        if rendered.toc.len() > 1 {
            toc.push_str("<p class=\"toc-title\">On this page</p>\n<ul>\n");
            for (level, id, text) in &rendered.toc {
                toc.push_str(&format!(
                    "<li class=\"toc-{level}\"><a href=\"#{id}\">{}</a></li>\n",
                    esc(text)
                ));
            }
            toc.push_str("</ul>\n");
        }

        let link = |p: &PageRef, class: &str, label: &str| {
            format!(
                "<a class=\"{class}\" href=\"{}\"><span>{label}</span>{}</a>",
                out_name(&p.file),
                esc(&p.title)
            )
        };
        let mut prevnext = String::new();
        if i > 0 {
            prevnext.push_str(&link(&pages[i - 1], "prev", "Previous"));
        } else {
            prevnext.push_str("<span></span>");
        }
        if let Some(next) = pages.get(i + 1) {
            prevnext.push_str(&link(next, "next", "Next"));
        }

        let mut vars = base.clone();
        vars.insert("root", "../".to_string());
        vars.insert("title", esc(&title));
        vars.insert("section", esc(&page.section));
        vars.insert("content", rendered.html.clone());
        vars.insert("sidebar", sidebar);
        vars.insert("toc", toc);
        vars.insert("prevnext", prevnext);
        vars.insert(
            "edit_url",
            format!("{REPO}/blob/main/{HANDBOOK}/{}", page.file),
        );
        write(
            &out.join("docs").join(&name),
            &templates.fill("doc.html", &vars)?,
        )?;

        for (id, heading, text) in &rendered.sections {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() && heading.is_empty() {
                continue;
            }
            let url = if id.is_empty() {
                format!("docs/{name}")
            } else {
                format!("docs/{name}#{id}")
            };
            search.push(format!(
                "{{\"u\":{},\"p\":{},\"h\":{},\"t\":{}}}",
                json_str(&url),
                json_str(&title),
                json_str(heading),
                json_str(&text)
            ));
        }
    }
    write(
        &out.join("assets").join("search.json"),
        &format!("[{}]", search.join(",\n")),
    )?;

    let checked = check_links(&out)?;
    println!(
        "site: {} handbook pages, {checked} internal links checked, written to {}",
        pages.len(),
        out.display()
    );
    println!(
        "preview: python3 -m http.server --directory {} 8000",
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handbook_links_become_pages_and_the_rest_points_at_the_repository() {
        let known: BTreeSet<String> = ["undo.md".to_string(), "README.md".to_string()].into();
        assert_eq!(rewrite_link("undo.md#limits", &known), "undo.html#limits");
        assert_eq!(rewrite_link("README.md", &known), "index.html");
        assert_eq!(
            rewrite_link("../SECURITY.md", &known),
            format!("{REPO}/blob/main/docs/SECURITY.md")
        );
        assert_eq!(
            rewrite_link("../../STATUS.md#x", &known),
            format!("{REPO}/blob/main/STATUS.md#x")
        );
        assert_eq!(
            rewrite_link("https://example.org/a.md", &known),
            "https://example.org/a.md"
        );
        assert_eq!(rewrite_link("#here", &known), "#here");
    }

    #[test]
    fn console_blocks_mark_commands_and_escape_output() {
        let html = console_html("$ yapacc why <pkg>  # a comment\nok    fine\nokay then\n");
        assert!(html.contains("<span class=\"p\">$</span>"));
        assert!(html.contains("yapacc why &lt;pkg&gt;"));
        assert!(html.contains("<span class=\"k\">  # a comment</span>"));
        assert!(html.contains("<span class=\"st-ok\">ok</span>"));
        assert!(html.contains("<span class=\"ln\">okay then</span>"));
    }

    #[test]
    fn headings_get_unique_anchors_and_the_first_h1_is_the_title() {
        let r = render(
            "# Title\n\n## Limits\n\ntext\n\n## Limits\n",
            &BTreeSet::new(),
        );
        assert_eq!(r.title, "Title");
        assert!(r.html.contains("id=\"limits\""));
        assert!(r.html.contains("id=\"limits-2\""));
        assert!(!r.html.contains("<h1"));
        assert_eq!(r.toc.len(), 2);
    }

    #[test]
    fn summary_lists_pages_under_their_section() {
        let pages = parse_summary(
            "# x\n\n## Start\n- [Install](install.md)\n## Use\n- [Undo](undo.md) — text\n",
        )
        .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].section, "Use");
        assert_eq!(pages[1].file, "undo.md");
        assert_eq!(out_name("README.md"), "index.html");
    }
}
