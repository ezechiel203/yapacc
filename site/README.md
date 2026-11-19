# The website

The source of <https://ezechiel203.github.io/yapacc/>: a landing page and
the handbook, as static files with no third-party request.

```console
$ cargo xtask site                                   # builds into target/site
$ python3 -m http.server --directory target/site 8000
```

| Path | What |
|---|---|
| `templates/landing.html` | the landing page |
| `templates/doc.html` | the layout of a handbook page |
| `templates/_head.html`, `_masthead.html`, `_footer.html` | shared parts, included with `{{> name}}` |
| `templates/404.html` | the not-found page |
| `demos/*.txt` | terminal captures shown on the landing page, included with `{{demo name}}` |
| `assets/site.css`, `assets/site.js` | the stylesheet and the script; no framework, no dependency |
| `assets/fonts/` | the three typefaces, with their licences |
| `../docs/handbook/` | the handbook pages, plain Markdown; `SUMMARY.md` gives their order |

| `publish.sh` | publishes the built site |

The generator is `crates/xtask/src/site.rs`. It renders the Markdown, adds
anchors and the table of contents, writes the search index, and checks
every internal link and anchor: a broken one fails the build.

## Publishing

```console
$ site/publish.sh
```

It builds the site and makes the built files the only commit of the branch
`gh-pages`, which GitHub Pages serves. Each publication replaces the one
before; the branch holds nothing else.

The workflow `.github/workflows/site.yml` deploys the same files without
the branch. It is started by hand, needs the Pages source set to "GitHub
Actions", and has never run; the site is published by the script.

## Rules for the content

- **Terminal captures are real.** The files in `demos/` and the examples in
  the handbook are output of the program, shortened only by replacing a run
  of lines with a line holding `…`. They come from one session on a booted
  Debian 13 virtual machine (the cloud image of 2026-09-14, so that two
  months of updates were pending), run as an ordinary user on a
  pseudo-terminal with the release build: the read commands, the gate
  answered yes and no, an upgrade of two packages and its undo, the full
  upgrade, the undo of the full upgrade, and a reboot. The one exception is
  the second example of the briefing page, which is from an `archlinux`
  container, for the news feed. When behaviour changes, capture them again;
  do not edit them to match.
- **No claim without evidence.** What the landing page says about support
  must match [STATUS.md](../STATUS.md).
- **No third-party request.** No font service, no analytics, no embedded
  content. The pages must read the same with JavaScript turned off.
- **Relative links only,** so the site can be served from any path. The
  one exception is the not-found page: the server answers with it at any
  depth, so its links start with the path of the published site
  (`SITE_BASE` in the generator).
