//! Offline software taxonomy + deterministic query-goal parsing.
//!
//! Purpose: "I know what I want but not what it's called" — aptitude-style
//! browsing (`yapacc tree`, TUI Tree tab) and meaning expansion on empty
//! searches. Everything is OFFLINE and DETERMINISTIC: a curated keyword-rule
//! tree, no AI, no network, auditable tables.
//!
//! Tree shape: top category → subcategory (leaf) → packages classified by
//! first-match keyword rules against name+description.

use serde::Serialize;

/// (top, sub, keywords) — first match wins; rules are matched against
/// "name description" lowercased. Order matters: specific before general.
/// (top, [(sub, keywords)])
#[allow(clippy::type_complexity)] // curated data table, not an API type
pub const TREE: &[(&str, &[(&str, &[&str])])] = &[
    (
        "development",
        &[
            (
                "compilers-interpreters",
                &[
                    "compiler",
                    "interpreter",
                    "gcc",
                    "clang",
                    "rustc",
                    "python3",
                    "perl interpreter",
                    "lua interpreter",
                    "ruby interpreter",
                    "php interpreter",
                    "openjdk",
                    "jre",
                    "jdk",
                    "scheme",
                    "racket",
                    "forth",
                ],
            ),
            (
                "build-tools",
                &[
                    "build tool",
                    "make",
                    "cmake",
                    "meson",
                    "ninja",
                    "autoconf",
                    "automake",
                    "pkgconf",
                    "pkg-config",
                    "mold",
                    "bazel",
                    "buck",
                    "scons",
                    "b2 build",
                ],
            ),
            (
                "debuggers-profilers",
                &[
                    "gdb",
                    "debugger",
                    "lldb",
                    "strace",
                    "ltrace",
                    "valgrind",
                    "profiler",
                    "perf ",
                    "flamegraph",
                    "sanitizer",
                    "heaptrack",
                ],
            ),
            (
                "ides",
                &[
                    "ide",
                    "integrated development",
                    "vscode",
                    "code-oss",
                    "eclipse",
                    "jetbrains",
                    "kdevelop",
                    "qt creator",
                ],
            ),
            (
                "editors-terminal",
                &[
                    "neovim",
                    "vim",
                    "emacs",
                    "helix",
                    "kakoune",
                    "micro editor",
                    "text editor",
                    "source code editor",
                ],
            ),
            (
                "vcs",
                &[
                    "version control",
                    "git",
                    "mercurial",
                    "subversion",
                    "fossil",
                    "darcs",
                    "pijul",
                ],
            ),
            (
                "patch-review",
                &[
                    "diff",
                    "patch tool",
                    "merge tool",
                    "three-way merge",
                    "meld",
                    "difftastic",
                ],
            ),
            (
                "static-analysis",
                &[
                    "linter",
                    "static analysis",
                    "clang-tidy",
                    "clippy",
                    "shellcheck",
                    "code analysis",
                    "sonar",
                ],
            ),
            (
                "testing",
                &[
                    "test framework",
                    "unit test",
                    "pytest",
                    "criterion",
                    "test runner",
                    "mock server",
                    "cypress",
                    "selenium",
                ],
            ),
            (
                "lang-rust",
                &["rust language", "cargo", "crates", "rustup", "wasm rust"],
            ),
            (
                "lang-python",
                &[
                    "python package",
                    "python module",
                    "pip ",
                    "pypi",
                    "python library",
                ],
            ),
            (
                "lang-javascript",
                &[
                    "javascript",
                    "nodejs",
                    "npm",
                    "typescript",
                    "deno",
                    "webpack",
                    "vite",
                    "esbuild",
                    "babel",
                ],
            ),
            (
                "lang-c-cpp",
                &[
                    "c++",
                    "cpp",
                    "c library",
                    "boost",
                    "qt5",
                    "qt6",
                    "gtk binding",
                ],
            ),
            ("lang-go", &["golang", "go module", "gopls"]),
            (
                "lang-jvm",
                &[
                    "java library",
                    "scala",
                    "kotlin",
                    "clojure",
                    "gradle",
                    "maven",
                    "jvm",
                ],
            ),
            ("lang-php", &["php ", "composer", "laravel"]),
            (
                "lang-ruby",
                &["ruby gem", "rails", "bundler", "ruby library"],
            ),
            ("lang-perl", &["perl module", "cpan"]),
            ("lang-lua", &["lua module", "luarocks"]),
            (
                "lang-haskell",
                &["haskell", "cabal", "stack haskell", "ghc"],
            ),
            (
                "lang-functional",
                &[
                    "ocaml",
                    "elixir",
                    "erlang",
                    "clojure",
                    "f#",
                    "scheme",
                    "common lisp",
                    "racket",
                ],
            ),
            ("lang-dotnet", &["dotnet", "c# library", "mono ", "nuget"]),
            (
                "libraries",
                &[
                    "library",
                    "bindings",
                    "headers",
                    "lib ",
                    "shared objects",
                    "runtime library",
                ],
            ),
            (
                "api-tools",
                &[
                    "rest client",
                    "postman",
                    "httpie",
                    "openapi",
                    "graphql",
                    "api testing",
                    "insomnia",
                ],
            ),
            (
                "regex-parsing",
                &[
                    "regex",
                    "parser generator",
                    "bison",
                    "flex",
                    "parsing library",
                    "jq",
                    "xml parser",
                    "json parser",
                ],
            ),
        ],
    ),
    (
        "network",
        &[
            (
                "browsers",
                &[
                    "browser",
                    "firefox",
                    "chromium",
                    "web browser",
                    "epiphany",
                    "qutebrowser",
                    "netsurf",
                    "lynx",
                    "w3m",
                ],
            ),
            (
                "mail-clients",
                &[
                    "mail client",
                    "mail user agent",
                    "thunderbird",
                    "mutt",
                    "neomutt",
                    "notmuch",
                    "alpine mail",
                    "sylpheed",
                    "claws",
                ],
            ),
            (
                "mail-servers",
                &[
                    "mail server",
                    "smtp server",
                    "imap server",
                    "postfix",
                    "dovecot",
                    "exim",
                    "sendmail",
                    "mta",
                ],
            ),
            (
                "chat-im",
                &[
                    "matrix client",
                    "xmpp",
                    "telegram",
                    "discord",
                    "signal",
                    "slack",
                    "messaging client",
                    "whatsapp",
                    "irc client",
                    "weechat",
                    "hexchat",
                    "irssi",
                ],
            ),
            (
                "voip-videoconf",
                &[
                    "voip",
                    "sip client",
                    "jitsi",
                    "zoom",
                    "mumble",
                    "teamtalk",
                    "video conferencing",
                ],
            ),
            (
                "download-managers",
                &[
                    "download manager",
                    "downloader",
                    "wget",
                    "curl",
                    "aria2",
                    "yt-dlp",
                    "youtube-dl",
                ],
            ),
            (
                "torrent",
                &[
                    "torrent",
                    "bittorrent",
                    "transmission",
                    "qbittorrent",
                    "rtorrent",
                    "deluge",
                ],
            ),
            (
                "file-transfer",
                &[
                    "ftp client",
                    "sftp",
                    "rsync",
                    "file transfer",
                    "scp",
                    "ncftp",
                    "lftp",
                ],
            ),
            (
                "remote-access",
                &[
                    "ssh client",
                    "remote desktop",
                    "vnc",
                    "rdp",
                    "xrdp",
                    "mosh",
                    "telnet",
                    "anydesk",
                    "rustdesk",
                ],
            ),
            (
                "network-analysis",
                &[
                    "wireshark",
                    "packet capture",
                    "sniffer",
                    "tcpdump",
                    "traceroute",
                    "network scanner",
                    "nmap",
                    "packet analysis",
                ],
            ),
            (
                "dns-tools",
                &[
                    "dns",
                    "dig",
                    "nslookup",
                    "resolv",
                    "dns server",
                    "unbound",
                    "dnsmasq",
                    "knot",
                    "powerdns",
                ],
            ),
            (
                "servers-web",
                &[
                    "web server",
                    "http server",
                    "nginx",
                    "apache httpd",
                    "caddy",
                    "lighttpd",
                    "static site server",
                ],
            ),
            (
                "servers-proxy",
                &[
                    "proxy server",
                    "reverse proxy",
                    "squid",
                    "privoxy",
                    "tinyproxy",
                    "haproxy",
                ],
            ),
            (
                "vpn",
                &[
                    "vpn",
                    "wireguard",
                    "openvpn",
                    "tailscale",
                    "tunnel",
                    "zerotier",
                    "mullvad",
                ],
            ),
            (
                "wifi-tools",
                &[
                    "wifi",
                    "wireless",
                    "wpa_supplicant",
                    "iwd",
                    "aircrack",
                    "wireless scanner",
                    "wicd",
                ],
            ),
            (
                "network-manager",
                &[
                    "network manager",
                    "networkmanager",
                    "connman",
                    "systemd-networkd",
                    "netplan",
                ],
            ),
            (
                "routing",
                &["routing daemon", "bird", "quagga", "openbgpd", "mpls"],
            ),
            (
                "news-feeds",
                &[
                    "rss",
                    "newsboat",
                    "feed reader",
                    "atom feed",
                    "podcast catcher",
                    "gpodder",
                ],
            ),
        ],
    ),
    (
        "security",
        &[
            (
                "cryptography",
                &[
                    "crypto",
                    "gnupg",
                    "gpg",
                    "openssl",
                    "certificate",
                    "hashing tool",
                    "age encryption",
                    "key derivation",
                    "luks",
                ],
            ),
            (
                "password-managers",
                &[
                    "password manager",
                    "keepass",
                    "password store",
                    "pass ",
                    "bitwarden",
                    "vaultwarden",
                    "vault client",
                ],
            ),
            (
                "vulnerability-scanners",
                &[
                    "vulnerability",
                    "nuclei",
                    "nikto",
                    "exploit framework",
                    "metasploit",
                    "pentest",
                    "nessus",
                ],
            ),
            (
                "intrusion-detection",
                &[
                    "intrusion detection",
                    "snort",
                    "suricata",
                    "fail2ban",
                    "ids ",
                    "hids",
                ],
            ),
            (
                "firewalls",
                &[
                    "firewall",
                    "iptables",
                    "nftables",
                    "ufw",
                    "firewalld",
                    "packet filter",
                ],
            ),
            (
                "forensics",
                &[
                    "forensic",
                    "autopsy",
                    "sleuthkit",
                    "disk image analysis",
                    "steganography",
                    "volatility",
                ],
            ),
            (
                "anti-malware",
                &[
                    "antivirus",
                    "clamav",
                    "malware",
                    "rootkit",
                    "chkrootkit",
                    "rkhunter",
                ],
            ),
            (
                "anonymity",
                &[
                    "tor ",
                    "tor browser",
                    "anonymizing",
                    "i2p",
                    "mixnet",
                    "onion routing",
                ],
            ),
            (
                "file-integrity",
                &[
                    "integrity check",
                    "aide",
                    "tripwire",
                    "checksum verifier",
                    "hashdeep",
                ],
            ),
            (
                "honeypots",
                &["honeypot", "cowrie", "kippo", "decoy server"],
            ),
            (
                "siem-logging",
                &[
                    "security audit",
                    "siem",
                    "auditd",
                    "ossec",
                    "wazuh",
                    "log analysis security",
                ],
            ),
        ],
    ),
    (
        "multimedia",
        &[
            (
                "image-editors",
                &[
                    "gimp",
                    "photoshop",
                    "image editor",
                    "photo editor",
                    "krita",
                    "painting program",
                    "mypaint",
                    "digital painting",
                ],
            ),
            (
                "image-viewers",
                &[
                    "image viewer",
                    "photo viewer",
                    "feh",
                    "sxiv",
                    "nsxiv",
                    "imv",
                    "gpicview",
                    "gallery viewer",
                    "gthumb",
                    "nomacs",
                ],
            ),
            (
                "raw-photo",
                &[
                    "raw image",
                    "darktable",
                    "rawtherapee",
                    "dng",
                    "raw processing",
                    "digiKam",
                    "photo workflow",
                    "lightroom",
                ],
            ),
            (
                "vector-graphics",
                &[
                    "inkscape",
                    "vector graphics",
                    "svg editor",
                    "illustrator",
                    "vector drawing",
                ],
            ),
            (
                "video-players",
                &[
                    "video player",
                    "mpv",
                    "vlc",
                    "media player",
                    "mplayer",
                    "celluloid",
                    "haruna",
                    "totem",
                ],
            ),
            (
                "video-editors",
                &[
                    "video editor",
                    "kdenlive",
                    "openshot",
                    "pitivi",
                    "shotcut",
                    "video cutting",
                    "olive editor",
                ],
            ),
            (
                "video-transcoding",
                &[
                    "transcode",
                    "encode video",
                    "ffmpeg",
                    "handbrake",
                    "video conversion",
                    "avconv",
                    "mkvtoolnix",
                ],
            ),
            (
                "audio-players",
                &[
                    "audio player",
                    "music player",
                    "mpd",
                    "ncmpcpp",
                    "cmus",
                    "spotify",
                    "audacious",
                    "elisa",
                    "lollypop",
                    "rhythmbox",
                ],
            ),
            (
                "audio-editors",
                &[
                    "audio editor",
                    "audacity",
                    "sound editor",
                    "waveform editor",
                    "reaper",
                    "ardour",
                ],
            ),
            (
                "music-production",
                &[
                    "daw",
                    "digital audio workstation",
                    "synthesizer",
                    "midi sequencer",
                    "lmms",
                    "hydrogen drum",
                    "bitwig",
                    "vst plugin",
                    "audio plugin",
                ],
            ),
            (
                "audio-conversion",
                &[
                    "audio conversion",
                    "convert audio",
                    "sox",
                    "opusenc",
                    "lame",
                    "flac converter",
                    "audio cd ripper",
                    "abcde",
                    "whipper",
                ],
            ),
            (
                "codecs",
                &[
                    "codec",
                    "decoder library",
                    "encoder library",
                    "x264",
                    "x265",
                    "libav",
                    "av1",
                    "vpx",
                ],
            ),
            (
                "screenshot",
                &[
                    "screenshot",
                    "screen capture",
                    "flameshot",
                    "maim",
                    "scrot",
                    "spectacle",
                    "grim",
                ],
            ),
            (
                "screencast",
                &[
                    "screencast",
                    "screen recorder",
                    "obs studio",
                    "screen recording",
                    "simplescreenrecorder",
                    "wf-recorder",
                ],
            ),
            (
                "streaming",
                &[
                    "streaming",
                    "obs",
                    "twitch",
                    "live stream",
                    "rtmp",
                    "icecast",
                    "streaming server",
                ],
            ),
            ("podcast", &["podcast", "podcast client", "gPodder", "cpod"]),
        ],
    ),
    (
        "graphics",
        &[
            (
                "3d-modeling",
                &[
                    "3d modeling",
                    "blender",
                    "mesh editing",
                    "3d model",
                    "sculpting",
                    "wavesurfer 3d",
                    "makehuman",
                ],
            ),
            (
                "3d-rendering",
                &[
                    "rendering engine",
                    "raytracing",
                    "raytrace",
                    "cycles render",
                    "luxrender",
                    "yafaray",
                    "poclain",
                ],
            ),
            (
                "cad",
                &[
                    "cad",
                    "freecad",
                    "openscad",
                    "autocad",
                    "drafting",
                    "technical drawing",
                    "librecad",
                    "qcad",
                    "brlcad",
                ],
            ),
            (
                "diagramming",
                &[
                    "diagram",
                    "drawio",
                    "draw.io",
                    "visio",
                    "flowchart",
                    "uml tool",
                    "plantuml",
                    "mermaid",
                    "graphviz",
                    "dia ",
                    "yed",
                ],
            ),
            (
                "plotting-visualization",
                &[
                    "plot",
                    "gnuplot",
                    "graphing",
                    "chart library",
                    "matplotlib",
                    "vega",
                    "data visualization",
                    "graph drawing",
                ],
            ),
            (
                "terminal-graphics",
                &[
                    "terminal draw",
                    "ascii art",
                    "caca",
                    "aalib",
                    "figlet",
                    "jp2a",
                    "tty plot",
                    "draw in terminal",
                    "terminal graphics",
                    "chafa",
                    "viu",
                    "terminal image",
                ],
            ),
            (
                "image-processing-cli",
                &[
                    "imagemagick",
                    "graphicsmagick",
                    "image processing",
                    "image conversion",
                    "batch resize",
                    "feh ",
                ],
            ),
            (
                "color-tools",
                &[
                    "color picker",
                    "color management",
                    "icc profile",
                    "argyll",
                    "displaycal",
                    "gamma correction",
                ],
            ),
            (
                "font-editors",
                &[
                    "font editor",
                    "fontforge",
                    "glyph design",
                    "typeface design",
                    "birdfont",
                ],
            ),
            (
                "panorama-hdr",
                &["panorama", "hugin", "hdr", "photo stitching", "enblend"],
            ),
            (
                "scanning-ocr-graphics",
                &["ocr", "tesseract", "image ocr", "gocr", "ocrmypdf"],
            ),
        ],
    ),
    (
        "office",
        &[
            (
                "word-processors",
                &[
                    "word processor",
                    "libreoffice writer",
                    "abiword",
                    "docx editor",
                    "calligra words",
                ],
            ),
            (
                "spreadsheets",
                &[
                    "spreadsheet",
                    "libreoffice calc",
                    "gnumeric",
                    "excel compatible",
                    "xlsx",
                    "calligra sheets",
                ],
            ),
            (
                "presentations",
                &[
                    "presentation",
                    "slides",
                    "libreoffice impress",
                    "beamer",
                    "powerpoint compatible",
                    "calligra stage",
                ],
            ),
            (
                "suites",
                &[
                    "libreoffice",
                    "office suite",
                    "calligra",
                    "onlyoffice",
                    "collabora",
                ],
            ),
            (
                "pdf-viewers",
                &[
                    "pdf viewer",
                    "zathura",
                    "mupdf",
                    "okular",
                    "evince",
                    "xpdf",
                    "apvlv",
                ],
            ),
            (
                "pdf-editors",
                &[
                    "pdf editor",
                    "pdf manipulation",
                    "pdftk",
                    "qpdf",
                    "pdf merge",
                    "pdf split",
                    "pdf arrange",
                ],
            ),
            (
                "notes",
                &[
                    "note taking",
                    "obsidian",
                    "joplin",
                    "journal",
                    "zettelkasten",
                    "taskwarrior",
                    "org-mode",
                    "todo manager",
                    "todotxt",
                ],
            ),
            (
                "calendar-pim",
                &[
                    "calendar",
                    "pim",
                    "personal information",
                    "khal",
                    "vdirsyncer",
                    "thunderbird calendar",
                    "gnome calendar",
                    "contacts",
                ],
            ),
            (
                "reference-dictionaries",
                &[
                    "dictionary",
                    "thesaurus",
                    "sdcv",
                    "stardict",
                    "goldendict",
                    "translation tool",
                    "wikipedia offline",
                    "wiktionary",
                ],
            ),
            (
                "ebooks",
                &[
                    "ebook",
                    "epub",
                    "calibre",
                    "mobi reader",
                    "book reader",
                    "fbreader",
                    "sigil",
                ],
            ),
            (
                "bibliography",
                &[
                    "bibliography",
                    "bibtex",
                    "zotero",
                    "jabref",
                    "citation manager",
                    "latex references",
                ],
            ),
            (
                "mindmaps",
                &[
                    "mind map",
                    "mindmapping",
                    "freemind",
                    "xmind",
                    "concept map",
                    "vym",
                ],
            ),
        ],
    ),
    (
        "science",
        &[
            (
                "math-symbolic",
                &[
                    "symbolic math",
                    "maxima",
                    "sagemath",
                    "mathematica",
                    "sympy",
                    "cas ",
                    "computer algebra",
                ],
            ),
            (
                "math-numeric",
                &[
                    "numerical",
                    "octave",
                    "scilab",
                    "lapack",
                    "blas",
                    "linear algebra",
                    "matrix computation",
                    "numpy",
                ],
            ),
            (
                "statistics",
                &[
                    "statistics",
                    "statistical",
                    "r language",
                    "rstudio",
                    "stata",
                    "regression analysis",
                    "jasp",
                    "pspp",
                    "julia stats",
                ],
            ),
            (
                "data-analysis",
                &[
                    "data analysis",
                    "jupyter",
                    "pandas",
                    "dataframe",
                    "data science",
                    "rstudio",
                    "data wrangling",
                    "polars",
                ],
            ),
            (
                "ai-ml",
                &[
                    "machine learning",
                    "neural network",
                    "deep learning",
                    "tensorflow",
                    "pytorch",
                    "onnx",
                    "llm",
                    "inference engine",
                    "stable diffusion",
                    "cuda",
                    "ollama",
                    "local ai",
                ],
            ),
            (
                "physics",
                &[
                    "physics simulation",
                    "particle physics",
                    "geant4",
                    "quantum simulator",
                    "celestial mechanics",
                ],
            ),
            (
                "chemistry",
                &[
                    "chemistry",
                    "molecule viewer",
                    "avogadro",
                    "chemtool",
                    "quantum chemistry",
                    "psi4",
                    "periodic table",
                ],
            ),
            (
                "bioinformatics",
                &[
                    "bioinformatics",
                    "sequence alignment",
                    "blast ",
                    "genomics",
                    "protein structure",
                    "biopython",
                    "gatk",
                    "samtools",
                ],
            ),
            (
                "astronomy",
                &[
                    "astronomy",
                    "stellarium",
                    "telescope control",
                    "planetarium",
                    "celestial",
                    "kstars",
                    "skychart",
                ],
            ),
            (
                "geospatial",
                &[
                    "gis",
                    "geospatial",
                    "qgis",
                    "grass gis",
                    "openstreetmap",
                    "postgis",
                    "map tool",
                    "gdal",
                    "proj",
                ],
            ),
            (
                "electronics-eda",
                &[
                    "eda",
                    "kicad",
                    "pcb design",
                    "schematic capture",
                    "circuit design",
                    "verilog",
                    "vhdl",
                    "spice",
                    "falstad",
                    "gschem",
                ],
            ),
            (
                "circuit-simulation",
                &[
                    "circuit simulation",
                    "ngspice",
                    "simulide",
                    "digital logic simulator",
                    "logisim",
                ],
            ),
            (
                "robotics",
                &[
                    "robotics",
                    "ros ",
                    "gazebo simulator",
                    "robot simulator",
                    "moveit",
                ],
            ),
        ],
    ),
    (
        "database",
        &[
            (
                "sql-servers",
                &[
                    "postgresql",
                    "mysql",
                    "mariadb",
                    "database server",
                    "sql server",
                    "percona",
                ],
            ),
            (
                "nosql",
                &[
                    "redis",
                    "mongodb",
                    "cassandra",
                    "key-value store",
                    "document database",
                    "elasticsearch",
                    "couchdb",
                    "neo4j",
                    "graph database",
                    "influxdb",
                    "timeseries",
                ],
            ),
            (
                "embedded-db",
                &[
                    "embedded database",
                    "sqlite",
                    "lmdb",
                    "rocksdb",
                    "leveldb",
                    "berkeley db",
                ],
            ),
            (
                "clients-cli",
                &[
                    "sql client",
                    "psql",
                    "mycli",
                    "pgcli",
                    "usql",
                    "database shell",
                    "duckdb",
                    "sql REPL",
                ],
            ),
            (
                "clients-gui",
                &[
                    "database gui",
                    "dbeaver",
                    "pgadmin",
                    "mysql workbench",
                    "beekeeper",
                    "baserow",
                ],
            ),
            (
                "migration-backup",
                &[
                    "database migration",
                    "db backup",
                    "pg_dump",
                    "barman",
                    "flyway",
                    "liquibase",
                    "dbmate",
                ],
            ),
            (
                "orm-drivers",
                &[
                    "orm",
                    "sqlalchemy",
                    "sequelize",
                    "gorm",
                    "database driver",
                    "odbc",
                    "jdbc driver",
                ],
            ),
        ],
    ),
    (
        "system",
        &[
            (
                "shells",
                &[
                    "shell",
                    "bash",
                    "zsh",
                    "fish shell",
                    "dash shell",
                    "ksh",
                    "nushell",
                    "oil shell",
                    "elvish",
                ],
            ),
            (
                "terminal-emulators",
                &[
                    "terminal emulator",
                    "alacritty",
                    "kitty terminal",
                    "foot terminal",
                    "xterm",
                    "st ",
                    "wezterm",
                    "konsole",
                    "gnome-terminal",
                    "tilix",
                    "terminator",
                ],
            ),
            (
                "terminal-multiplexers",
                &["terminal multiplexer", "tmux", "screen ", "zellij", "byobu"],
            ),
            (
                "file-managers",
                &[
                    "file manager",
                    "ranger",
                    "nnn",
                    "vifm",
                    "midnight commander",
                    "thunar",
                    "dolphin",
                    "nautilus",
                    "nemo",
                    "pcmanfm",
                    "lf ",
                    "yazi",
                    "mc ",
                ],
            ),
            (
                "launchers",
                &[
                    "application launcher",
                    "rofi",
                    "dmenu",
                    "wofi",
                    "fzf launcher",
                    "ulauncher",
                    "albert",
                    "krunner",
                ],
            ),
            (
                "archiving",
                &[
                    "archive",
                    "compression",
                    "tar ",
                    "zip tool",
                    "7zip",
                    "p7zip",
                    "unrar",
                    "zstd",
                    "xz tool",
                    "lz4",
                    "brotli",
                    "zpaq",
                ],
            ),
            (
                "monitoring",
                &[
                    "monitor",
                    "htop",
                    "btop",
                    "bpytop",
                    "glances",
                    "system monitor",
                    "resource usage",
                    "top ",
                    "atop",
                    "iotop",
                    "bandwhich",
                ],
            ),
            (
                "disk-partitioning",
                &[
                    "disk partition",
                    "gdisk",
                    "fdisk",
                    "parted",
                    "partition manager",
                    "gparted",
                    "partition table",
                ],
            ),
            (
                "filesystems-tools",
                &[
                    "filesystem",
                    "btrfs",
                    "zfs",
                    "ext4 tool",
                    "xfs",
                    "f2fs",
                    "filesystem checker",
                    "fsck",
                    "smartctl",
                    "disk usage",
                    "du ",
                    "ncdu",
                    "dust",
                    "df ",
                    "dysk",
                ],
            ),
            (
                "backup",
                &[
                    "backup",
                    "borg",
                    "restic",
                    "kopia",
                    "duplicity",
                    "rsnapshot",
                    "timeshift",
                    "backup daemon",
                    "bacula",
                    "amanda",
                ],
            ),
            (
                "boot-loaders",
                &[
                    "boot loader",
                    "grub",
                    "systemd-boot",
                    "refind",
                    "limine",
                    "syslinux",
                    "boot manager",
                ],
            ),
            (
                "init-system",
                &[
                    "init system",
                    "systemd",
                    "openrc",
                    "runit",
                    "s6 ",
                    "service manager",
                    "supervisord",
                ],
            ),
            (
                "kernel-related",
                &[
                    "kernel module",
                    "kernel headers",
                    "dkms",
                    "kernel image",
                    "linux firmware",
                    "module loader",
                ],
            ),
            (
                "firmware",
                &["firmware", "uefi tool", "fwupd", "bios update", "edk2"],
            ),
            (
                "hardware-detection",
                &[
                    "hardware detection",
                    "lshw",
                    "hwinfo",
                    "lsusb",
                    "lspci",
                    "inxi",
                    "dmidecode",
                    "hw-probe",
                ],
            ),
            (
                "power-management",
                &[
                    "power management",
                    "tlp",
                    "powertop",
                    "upower",
                    "battery",
                    "suspend",
                    "hibernate",
                    "acpid",
                ],
            ),
            (
                "printing",
                &[
                    "cups",
                    "printing",
                    "print server",
                    "lp ",
                    "hplip",
                    "printer driver",
                ],
            ),
            (
                "scanning",
                &["scanner", "sane", "scan tool", "simple-scan", "skanlite"],
            ),
            ("bluetooth", &["bluetooth", "bluez", "bluetoothctl", "obex"]),
            (
                "usb-thunderbolt",
                &["usb tool", "thunderbolt", "usbmuxd", "libusb tool"],
            ),
            (
                "time-date",
                &[
                    "ntp",
                    "time synchronization",
                    "chrony",
                    "ntpdate",
                    "timezone",
                    "clock sync",
                ],
            ),
            (
                "locale-i18n",
                &[
                    "locale",
                    "i18n",
                    "internationalization",
                    "unicode data",
                    "gettext",
                    "fcitx",
                    "ibus",
                ],
            ),
            (
                "virtual-filesystems",
                &[
                    "fuse",
                    "sshfs",
                    "nfs client",
                    "cifs",
                    "samba client",
                    "webdav",
                    "rclone mount",
                    "gvfs",
                ],
            ),
        ],
    ),
    (
        "desktop",
        &[
            (
                "window-managers",
                &[
                    "window manager",
                    "i3",
                    "sway",
                    "hyprland",
                    "bspwm",
                    "dwm",
                    "awesome wm",
                    "openbox",
                    "fluxbox",
                    "xmonad",
                    "herbstluftwm",
                    "wm ",
                ],
            ),
            (
                "desktop-environments",
                &[
                    "desktop environment",
                    "gnome-shell",
                    "kde plasma",
                    "xfce",
                    "lxqt",
                    "lxde",
                    "cinnamon",
                    "mate desktop",
                    "budgie",
                ],
            ),
            (
                "display-managers",
                &[
                    "display manager",
                    "login manager",
                    "sddm",
                    "gdm",
                    "lightdm",
                    "ly ",
                    "greetd",
                ],
            ),
            (
                "themes-icons",
                &[
                    "theme",
                    "icon theme",
                    "gtk theme",
                    "qt theme",
                    "cursor theme",
                    "wallpaper",
                    "customization",
                    "nord theme",
                    "catppuccin",
                    "papirus",
                ],
            ),
            (
                "panels-docks",
                &[
                    "panel",
                    "dock",
                    "taskbar",
                    "waybar",
                    "polybar",
                    "plank",
                    "latte-dock",
                    "tint2",
                    "xfce4-panel",
                ],
            ),
            (
                "notifications",
                &[
                    "notification",
                    "dunst",
                    "mako",
                    "notify-send",
                    "notification daemon",
                    "swaync",
                ],
            ),
            (
                "lock-screens",
                &[
                    "lock screen",
                    "screen locker",
                    "i3lock",
                    "swaylock",
                    "gtklock",
                    "xsecurelock",
                ],
            ),
            (
                "display-configuration",
                &[
                    "display configuration",
                    "xrandr",
                    "wlr-randr",
                    "monitor layout",
                    "screen resolution",
                    "redshift",
                    "gamma",
                    "brightness",
                ],
            ),
            (
                "input-methods",
                &[
                    "input method",
                    "fcitx",
                    "ibus",
                    "keyboard layout",
                    "xkb",
                    "keyd",
                    "remap",
                    "touchpad",
                ],
            ),
            (
                "accessibility",
                &[
                    "accessibility",
                    "screen reader",
                    "orca",
                    "magnifier",
                    "on-screen keyboard",
                    "a11y",
                ],
            ),
            (
                "clipboard",
                &[
                    "clipboard",
                    "copyq",
                    "wl-clipboard",
                    "xclip",
                    "clipman",
                    "greenclip",
                    "clipboard manager",
                ],
            ),
            (
                "file-dialogs-portals",
                &[
                    "xdg portal",
                    "file dialog",
                    "xdg-desktop",
                    "gtk filechooser",
                ],
            ),
        ],
    ),
    (
        "cli",
        &[
            (
                "tui-apps",
                &[
                    "tui",
                    "terminal user interface",
                    "ncurses",
                    "curses",
                    "terminal app",
                    "ratatui",
                    "terminal dashboard",
                ],
            ),
            (
                "file-search",
                &[
                    "search tool",
                    "ripgrep",
                    "fd ",
                    "fzf",
                    "find files",
                    "grep tool",
                    "locate files",
                    "ugrep",
                    "ast-grep",
                ],
            ),
            (
                "text-processing",
                &[
                    "sed ",
                    "awk",
                    "text processing",
                    "cut tool",
                    "column",
                    "csv tool",
                    "mill",
                    "text transform",
                    "sd ",
                    "choose",
                ],
            ),
            (
                "terminal-utilities",
                &[
                    "terminal utility",
                    "cli tool",
                    "command-line",
                    "command line tool",
                    "coreutils",
                    "busybox",
                ],
            ),
            (
                "file-sync",
                &[
                    "sync tool",
                    "unison",
                    "syncthing",
                    "file synchronization",
                    "rclone",
                    "freefilesync",
                ],
            ),
            (
                "calculators",
                &[
                    "calculator",
                    "bc ",
                    "qalc",
                    "rofi-calc",
                    "numbat",
                    "unit conversion",
                ],
            ),
            (
                "batch-automation",
                &[
                    "automation",
                    "task runner",
                    "just ",
                    "make alternative",
                    "cron tool",
                    "watchexec",
                    "entr",
                    "parallel",
                    "xargs tool",
                ],
            ),
            (
                "prompt-shell-integration",
                &[
                    "prompt",
                    "starship",
                    "powerline",
                    "zsh plugin",
                    "bash completion",
                    "fish plugin",
                    "atuin",
                ],
            ),
        ],
    ),
    (
        "games",
        &[
            (
                "action",
                &[
                    "action game",
                    "shooter",
                    "fps game",
                    "doom",
                    "quake",
                    "platformer",
                    "side-scroller",
                    "beat-em-up",
                ],
            ),
            (
                "strategy-rts",
                &[
                    "real-time strategy",
                    "rts",
                    "strategy game",
                    "openra",
                    "0ad",
                    "warcraft",
                    "command & conquer",
                ],
            ),
            (
                "strategy-turnbased",
                &[
                    "turn-based strategy",
                    "civilization",
                    "4x game",
                    "hex strategy",
                    "freeciv",
                    "wesnoth",
                ],
            ),
            (
                "simulation",
                &[
                    "simulator game",
                    "flight simulator",
                    "city building",
                    "simcity",
                    "openTTD",
                    "transport simulation",
                    "farming simulator",
                ],
            ),
            (
                "racing",
                &[
                    "racing game",
                    "car racing",
                    "supertux kart",
                    "speed dreams",
                    "trigger rally",
                    "stunt rally",
                ],
            ),
            (
                "sports",
                &[
                    "sports game",
                    "football",
                    "soccer game",
                    "basketball",
                    "tennis game",
                    "golf game",
                    "chess boxing",
                ],
            ),
            (
                "puzzle",
                &[
                    "puzzle game",
                    "sokoban",
                    "minesweeper",
                    "tetris",
                    "match-3",
                    "2048",
                    "logic puzzle",
                ],
            ),
            (
                "card-board",
                &[
                    "card game",
                    "board game",
                    "chess engine",
                    "go game",
                    "gnuchess",
                    "stockfish",
                    "solitaire",
                    "mahjong",
                    "poker",
                ],
            ),
            (
                "roguelike",
                &[
                    "roguelike",
                    "dungeon crawl",
                    "dcss",
                    "nethack",
                    "angband",
                    "crawl",
                    "cataclysm",
                ],
            ),
            (
                "mmo-mud",
                &["mud client", "mmo", "multiplayer game client", "openmw"],
            ),
            (
                "emulation",
                &[
                    "emulator",
                    "retroarch",
                    "dosbox",
                    "wine games",
                    "virtual console",
                    "mame",
                    "snes",
                    "genesis",
                    "pcsx",
                ],
            ),
            (
                "game-launchers",
                &[
                    "game launcher",
                    "steam",
                    "lutris",
                    "heroic",
                    "bottles",
                    "gamehub",
                    "legendary",
                ],
            ),
            (
                "game-tools",
                &[
                    "gamepad tool",
                    "controller mapping",
                    "antimicrox",
                    "game server",
                    "mod manager",
                    "cheat engine",
                ],
            ),
            (
                "educational-games",
                &[
                    "educational game",
                    "typing game",
                    "math game",
                    "gcompris",
                    "tux typing",
                ],
            ),
        ],
    ),
    (
        "fonts",
        &[
            (
                "sans-serif",
                &[
                    "sans",
                    "noto sans",
                    "dejavu",
                    "liberation sans",
                    "inter font",
                    "roboto",
                ],
            ),
            (
                "serif",
                &[
                    "serif",
                    "times font",
                    "georgia",
                    "garamond",
                    "noto serif",
                    "liberation serif",
                ],
            ),
            (
                "monospace",
                &[
                    "monospace",
                    "mono font",
                    "nerd font",
                    "fira code",
                    "jetbrains mono",
                    "coding font",
                    "hack font",
                    "iosevka",
                    "cascadia",
                ],
            ),
            (
                "symbols-emoji",
                &[
                    "emoji",
                    "symbol font",
                    "icon font",
                    "dingbat",
                    "noto emoji",
                    "math symbols",
                ],
            ),
            (
                "cjk-international",
                &[
                    "cjk font",
                    "chinese font",
                    "japanese font",
                    "korean font",
                    "arabic font",
                    "hebrew font",
                    "indic font",
                ],
            ),
        ],
    ),
    (
        "docs-knowledge",
        &[
            (
                "man-pages",
                &["man pages", "manpage", "manual pages", "man-db"],
            ),
            (
                "doc-generators",
                &[
                    "documentation generator",
                    "sphinx",
                    "doxygen",
                    "mdbook",
                    "rustdoc",
                    "javadoc",
                    "mkdocs",
                    "hugo docs",
                    "zola",
                ],
            ),
            (
                "wikis",
                &[
                    "wiki engine",
                    "mediawiki",
                    "dokuwiki",
                    "tiddlywiki",
                    "wiki client",
                ],
            ),
            (
                "cheatsheets",
                &[
                    "cheatsheet",
                    "tldr",
                    "cheat ",
                    "examples for commands",
                    "navi",
                ],
            ),
            (
                "knowledge-management",
                &[
                    "knowledge base",
                    "personal wiki",
                    "zettelkasten",
                    "notebook system",
                    "logseq",
                    "anki",
                    "flashcard",
                    "spaced repetition",
                ],
            ),
            (
                "bookmark-managers",
                &[
                    "bookmark",
                    "linkding",
                    "wallabag",
                    "read-it-later",
                    "pocket",
                    "shiori",
                ],
            ),
            (
                "forums-helpdesk",
                &[
                    "forum software",
                    "helpdesk",
                    "ticketing",
                    "osTicket",
                    "flarum",
                ],
            ),
        ],
    ),
    (
        "containers-virtualization",
        &[
            (
                "container-engines",
                &[
                    "docker",
                    "podman",
                    "container runtime",
                    "lxc",
                    "oci runtime",
                    "containerd",
                    "cri-o",
                    "distrobox",
                    "toolbox container",
                ],
            ),
            (
                "orchestration",
                &[
                    "kubernetes",
                    "kubectl",
                    "helm chart",
                    "compose file",
                    "swarm",
                    "nomad",
                    "minikube",
                    "k3s",
                    "k9s",
                ],
            ),
            (
                "virtual-machines",
                &[
                    "virtualbox",
                    "qemu",
                    "kvm",
                    "vmware",
                    "virtual machine",
                    "hypervisor",
                    "libvirt",
                    "virt-manager",
                    "gnome-boxes",
                    "quickemu",
                ],
            ),
            (
                "sandboxing",
                &[
                    "bubblewrap",
                    "bwrap",
                    "sandbox",
                    "firejail",
                    "namespaces",
                    "chroot tool",
                    "proot",
                    "systemd-nspawn",
                ],
            ),
            (
                "image-tools",
                &[
                    "container image",
                    "oci image",
                    "skopeo",
                    "buildah",
                    "kaniko",
                    "dockerfile lint",
                    "dive ",
                ],
            ),
            (
                "provisioners",
                &[
                    "terraform",
                    "ansible",
                    "puppet",
                    "chef ",
                    "salt stack",
                    "cloud-init",
                    "vagrant",
                    "packer",
                ],
            ),
        ],
    ),
    (
        "devops",
        &[
            (
                "ci-cd",
                &[
                    "continuous integration",
                    "ci/cd",
                    "jenkins",
                    "gitlab runner",
                    "github actions",
                    "drone ci",
                    "woodpecker",
                    "concourse",
                    "argocd",
                ],
            ),
            (
                "iac",
                &[
                    "infrastructure as code",
                    "terraform",
                    "pulumi",
                    "cloudformation",
                    "cdk",
                ],
            ),
            (
                "monitoring-observability",
                &[
                    "prometheus",
                    "grafana",
                    "observability",
                    "metrics dashboard",
                    "zabbix",
                    "nagios",
                    "sensu",
                    "victoriametrics",
                    "influxdb monitoring",
                    "opentelemetry",
                    "jaeger",
                    "tracing",
                ],
            ),
            (
                "log-management",
                &[
                    "log aggregation",
                    "log analysis",
                    "loki",
                    "graylog",
                    "fluentd",
                    "logstash",
                    "vector log",
                    "journalctl tool",
                    "lnav",
                    "goaccess",
                ],
            ),
            (
                "service-mesh",
                &[
                    "service mesh",
                    "istio",
                    "linkerd",
                    "envoy",
                    "consul connect",
                ],
            ),
            (
                "api-gateways",
                &["api gateway", "kong ", "traefik", "envoy proxy", "krakend"],
            ),
            (
                "secrets-management",
                &[
                    "secrets management",
                    "vault ",
                    "sops",
                    "sealed secrets",
                    "infisical",
                ],
            ),
        ],
    ),
    (
        "education",
        &[
            (
                "language-learning",
                &[
                    "language learning",
                    "vocabulary",
                    "anki deck",
                    "duolingo alt",
                    "flashcard language",
                    "lingotrain",
                ],
            ),
            (
                "typing-tutors",
                &[
                    "typing tutor",
                    "typing practice",
                    "klavaro",
                    "ktouch",
                    "typing speed",
                    "monkeytype",
                ],
            ),
            (
                "math-training",
                &[
                    "math training",
                    "mental math",
                    "arithmetic practice",
                    "gcompris math",
                ],
            ),
            (
                "teaching-tools",
                &[
                    "teaching",
                    "classroom",
                    "presentation for teaching",
                    "smartboard",
                    "italc",
                ],
            ),
            (
                "courseware",
                &[
                    "course",
                    "mooc client",
                    "e-learning",
                    "moodle",
                    "chalkboard",
                ],
            ),
        ],
    ),
    (
        "communications",
        &[
            ("irc", &["irc"]),
            ("usenet", &["usenet", "nntp", "newsreader"]),
            ("fax-pager", &["fax", "pager", "hylafax"]),
        ],
    ),
];

/// Deterministic query-goal parser: intent words → facet categories,
/// significant tokens → search keywords, synonyms expand the keywords.
#[derive(Clone, Debug, Serialize)]
pub struct SearchGoal {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub synonyms_applied: Vec<String>,
}

const STOPWORDS: &[&str] = &[
    "in",
    "the",
    "with",
    "for",
    "a",
    "an",
    "to",
    "of",
    "and",
    "or",
    "my",
    "that",
    "will",
    "let",
    "me",
    "i",
    "want",
    "need",
    "some",
    "software",
    "program",
    "app",
    "package",
    "tool",
    "tools",
    "something",
    "can",
    "use",
    "using",
    "on",
    "at",
    "be",
    "is",
    "are",
];

/// intent word → (category, subcategory) hints + keyword synonyms
const INTENT: &[(&str, &[&str], &[&str])] = &[
    // word, category paths, keyword synonyms
    (
        "draw",
        &[
            "graphics.terminal-graphics",
            "multimedia.image-editors",
            "graphics.diagramming",
        ],
        &["draw", "paint", "sketch", "ascii"],
    ),
    (
        "paint",
        &["multimedia.image-editors"],
        &["paint", "draw", "gimp", "krita"],
    ),
    (
        "edit image",
        &["multimedia.image-editors"],
        &["gimp", "krita", "image editor"],
    ),
    (
        "edit video",
        &["multimedia.video-editors"],
        &["kdenlive", "video editor"],
    ),
    (
        "edit audio",
        &["multimedia.audio-editors"],
        &["audacity", "audio editor"],
    ),
    (
        "watch",
        &["multimedia.video-players"],
        &["mpv", "vlc", "video player"],
    ),
    (
        "listen",
        &["multimedia.audio-players"],
        &["music player", "mpd", "cmus"],
    ),
    (
        "browse",
        &["network.browsers"],
        &["firefox", "browser", "chromium"],
    ),
    ("mail", &["network.mail"], &["mail", "thunderbird", "mutt"]),
    ("chat", &["network.chat"], &["irc", "matrix", "chat"]),
    (
        "password",
        &["security.password-management"],
        &["pass", "keepass", "password"],
    ),
    (
        "encrypt",
        &["security.cryptography"],
        &["gnupg", "openssl", "encrypt"],
    ),
    ("vpn", &["network.vpn"], &["wireguard", "openvpn"]),
    ("backup", &["system.backup"], &["borg", "restic", "backup"]),
    (
        "monitor",
        &["system.monitoring"],
        &["htop", "btop", "monitor"],
    ),
    (
        "terminal",
        &["system.terminal-emulators", "cli.tui-apps"],
        &["terminal", "alacritty", "kitty"],
    ),
    ("shell", &["system.shells"], &["bash", "zsh", "fish"]),
    (
        "file manager",
        &["system.file-managers"],
        &["ranger", "nnn", "file manager"],
    ),
    (
        "search files",
        &["cli.file-search"],
        &["ripgrep", "fd", "fzf"],
    ),
    (
        "spreadsheet",
        &["office.spreadsheets"],
        &["calc", "gnumeric", "spreadsheet"],
    ),
    (
        "word",
        &["office.word-processors"],
        &["writer", "abiword", "word processor"],
    ),
    ("pdf", &["office.pdf"], &["zathura", "mupdf", "pdf"]),
    ("note", &["office.notes"], &["obsidian", "joplin", "notes"]),
    (
        "dictionary",
        &["office.reference"],
        &["dict", "sdcv", "dictionary"],
    ),
    ("3d", &["graphics.3d"], &["blender", "3d", "modeling"]),
    ("cad", &["graphics.cad"], &["freecad", "openscad", "cad"]),
    (
        "diagram",
        &["graphics.diagramming"],
        &["drawio", "plantuml", "diagram"],
    ),
    (
        "plot",
        &["graphics.plotting"],
        &["gnuplot", "chart", "plot"],
    ),
    ("game", &["games"], &["game"]),
    (
        "emulator",
        &["games.emulation"],
        &["retroarch", "dosbox", "emulator"],
    ),
    ("docker", &["containers.engines"], &["docker", "podman"]),
    (
        "vm",
        &["containers.virtualization"],
        &["qemu", "virtualbox", "virtual machine"],
    ),
    (
        "database",
        &["database"],
        &["postgres", "sqlite", "mariadb"],
    ),
    ("font", &["fonts"], &["font", "nerd"]),
];

pub fn parse_goal(query: &str) -> SearchGoal {
    let ql = query.to_lowercase();
    let mut goal = SearchGoal {
        categories: Vec::new(),
        keywords: Vec::new(),
        synonyms_applied: Vec::new(),
    };
    for (word, cats, syns) in INTENT {
        if ql.contains(word) {
            for c in cats.iter() {
                if !goal.categories.contains(&c.to_string()) {
                    goal.categories.push(c.to_string());
                }
            }
            for s in syns.iter() {
                if !goal.synonyms_applied.contains(&s.to_string()) {
                    goal.synonyms_applied.push(s.to_string());
                }
            }
        }
    }
    for token in ql.split_whitespace() {
        if token.len() > 2
            && !STOPWORDS.contains(&token)
            && !goal.keywords.iter().any(|k| k == token)
        {
            goal.keywords.push(token.to_string());
        }
    }
    goal
}

/// Classify a package into "category.subcategory" (first match; "" if none).
/// Two passes: name-only keywords first (a package NAMED "ripgrep" outranks a
/// description mentioning "regex"), then name+description.
///
/// **Performance:** Aho-Corasick single-pass keyword matching — the automaton
/// is built once from all keywords (~2000), each input is scanned in
/// O(input_length) regardless of keyword count. The old version did ~2000
/// individual `String::contains()` calls + cloned the leaf table per call
/// (575 µs/pkg in release, 2258 µs in debug); now ~100ns/pkg.
pub fn classify_pkg(name: &str, desc: &str) -> String {
    let cached = cached_leaves();
    let name_lower = name.to_lowercase();

    // pass 1: name-only — scan once, find ALL matches, pick min leaf index
    let mut best: Option<usize> = None;
    for mat in cached.ac.find_iter(&name_lower) {
        let leaf = cached.keyword_to_leaf[mat.pattern().as_usize()];
        if best.is_none_or(|b| leaf < b) {
            best = Some(leaf);
        }
        if best == Some(0) {
            break;
        } // can't do better than the first leaf
    }
    if let Some(leaf) = best {
        return cached.leaf_paths[leaf].clone();
    }

    // pass 2: name+desc (only when desc is non-empty)
    if !desc.is_empty() {
        let hay = format!("{name_lower} {}", desc.to_lowercase());
        for mat in cached.ac.find_iter(&hay) {
            let leaf = cached.keyword_to_leaf[mat.pattern().as_usize()];
            if best.is_none_or(|b| leaf < b) {
                best = Some(leaf);
            }
            if best == Some(0) {
                break;
            }
        }
    }
    if let Some(leaf) = best {
        return cached.leaf_paths[leaf].clone();
    }

    // name-based fallbacks
    if name_lower.starts_with("ttf-")
        || name_lower.starts_with("otf-")
        || name_lower.starts_with("font")
    {
        return "fonts".into();
    }
    if name_lower.starts_with("lib")
        || name_lower.starts_with("python-")
        || name_lower.starts_with("perl-")
        || name_lower.starts_with("ruby-")
        || name_lower.starts_with("nodejs-")
        || name_lower.starts_with("julia-")
    {
        return "development.libraries".into();
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Cached leaf table with Aho-Corasick automaton (RwLock<Option<Arc>>)
// ---------------------------------------------------------------------------

struct CachedLeaves {
    /// Aho-Corasick automaton over all lowercased keywords
    ac: aho_corasick::AhoCorasick,
    /// keyword pattern index → leaf index (parallel to patterns given to AC)
    keyword_to_leaf: Vec<usize>,
    /// leaf path strings (indexed by leaf index)
    leaf_paths: Vec<String>,
}

static CACHE: std::sync::RwLock<Option<std::sync::Arc<CachedLeaves>>> =
    std::sync::RwLock::new(None);

fn cached_leaves() -> std::sync::Arc<CachedLeaves> {
    // fast path: read lock, clone Arc (~5ns)
    if let Ok(guard) = CACHE.read()
        && let Some(c) = guard.as_ref()
    {
        return std::sync::Arc::clone(c);
    }
    // build and cache (write lock)
    let raw = effective_leaves();
    let mut patterns_lower: Vec<String> = Vec::new();
    let mut keyword_to_leaf: Vec<usize> = Vec::new();
    let mut leaf_paths: Vec<String> = Vec::with_capacity(raw.len());
    for (leaf_idx, (top, sub, keywords)) in raw.iter().enumerate() {
        leaf_paths.push(format!("{top}.{sub}"));
        for k in keywords {
            patterns_lower.push(k.to_lowercase());
            keyword_to_leaf.push(leaf_idx);
        }
    }
    let patterns: Vec<&str> = patterns_lower.iter().map(|s| s.as_str()).collect();
    // build with `LeftmostFirst`: when patterns overlap at a position,
    // the one added first (lower leaf index) wins
    let ac = aho_corasick::AhoCorasick::builder()
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build(&patterns)
        .expect("aho-corasick pattern build");
    let c = std::sync::Arc::new(CachedLeaves {
        ac,
        keyword_to_leaf,
        leaf_paths,
    });
    if let Ok(mut guard) = CACHE.write()
        && guard.is_none()
    {
        *guard = Some(std::sync::Arc::clone(&c));
    }
    c
}

/// Invalidate the classify_pkg cache (used when the external taxonomy changes).
fn invalidate_cache() {
    if let Ok(mut guard) = CACHE.write() {
        *guard = None;
    }
}

/// Optional external taxonomy override (e.g. exported/synced from the
/// knowledge service at release time). Format: TOML list of
/// [leaf] top/sub/keywords entries. Runtime stays offline+fast; this is a
/// curation import path, not a runtime dependency.
#[derive(serde::Deserialize)]
struct LeafToml {
    top: String,
    sub: String,
    keywords: Vec<String>,
}

#[derive(serde::Deserialize)]
struct TaxonomyFile {
    leaf: Vec<LeafToml>,
}

pub fn import_toml(text: &str) -> Result<usize, String> {
    let leaves: Vec<LeafToml> = toml::from_str::<TaxonomyFile>(text)
        .map_err(|e| format!("taxonomy.toml parse error: {e}"))?
        .leaf;
    for l in &leaves {
        if l.top.is_empty() || l.sub.is_empty() || l.keywords.is_empty() {
            return Err(format!("leaf {}.{} lacks top/sub/keywords", l.top, l.sub));
        }
    }
    let n = leaves.len();
    invalidate_cache(); // classify_pkg cache must reflect the new leaves
    *EXTERNAL.leaves.lock().unwrap() = Some(
        leaves
            .into_iter()
            .map(|l| (l.top, l.sub, l.keywords))
            .collect::<Vec<(String, String, Vec<String>)>>(),
    );
    Ok(n)
}

fn external_path() -> Option<std::path::PathBuf> {
    let xdg = crate::config::xdg_base("XDG_CONFIG_HOME", ".config")?;
    let p = xdg.join("yapacc/taxonomy.toml");
    p.is_file().then_some(p)
}

/// Effective leaves: external taxonomy.toml when present, else the built-in
/// TREE. Loaded once per process.
pub type OwnedLeaf = (String, String, Vec<String>);

pub fn effective_leaves() -> Vec<OwnedLeaf> {
    {
        let guard = EXTERNAL.leaves.lock().unwrap();
        if let Some(l) = guard.as_ref() {
            return l.clone();
        }
    }
    // A taxonomy file that cannot be used is said once, by name, and is not
    // read again in this process: the built-in categories then apply.
    if !EXTERNAL_REFUSED.load(std::sync::atomic::Ordering::Relaxed)
        && let Some(p) = external_path()
    {
        match load_external(&p) {
            Ok(n) => {
                eprintln!("note: using external taxonomy {} ({n} leaves)", p.display());
                return effective_leaves();
            }
            Err(e) => {
                if !EXTERNAL_REFUSED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("{}", external_warning(&p, &e));
                }
            }
        }
    }
    TREE.iter()
        .flat_map(|(top, subs)| {
            subs.iter().map(|(sub, keys)| {
                (
                    top.to_string(),
                    sub.to_string(),
                    keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
                )
            })
        })
        .collect()
}

/// The user's taxonomy file was found unusable (and the warning printed).
static EXTERNAL_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read and install the user's taxonomy file; the number of leaves.
fn load_external(path: &std::path::Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot be read: {e}"))?;
    import_toml(&text)
}

/// What is printed, once, about a taxonomy file that is not used.
fn external_warning(path: &std::path::Path, why: &str) -> String {
    format!(
        "warning: {}: this taxonomy file is not used ({why}); the built-in categories apply",
        path.display()
    )
}

static EXTERNAL: ExternalOnce = ExternalOnce {
    leaves: std::sync::Mutex::new(None),
};
struct ExternalOnce {
    leaves: std::sync::Mutex<Option<Vec<OwnedLeaf>>>,
}

/// All top-level categories.
pub fn tops() -> Vec<String> {
    let mut t: Vec<String> = effective_leaves().into_iter().map(|(t, _, _)| t).collect();
    t.sort();
    t.dedup();
    t
}

/// Subcategories of a top category.
pub fn subs(top: &str) -> Option<Vec<String>> {
    let mut s: Vec<String> = effective_leaves()
        .into_iter()
        .filter(|(t, _, _)| t == top)
        .map(|(_, s, _)| s)
        .collect();
    s.sort();
    s.dedup();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test replaces the process-wide taxonomy; every test that reads it
    // takes this lock so they cannot observe each other.
    static TAXONOMY: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn goal_parsing_draw_in_terminal() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        let g = parse_goal("list all software that let me draw in the terminal with my keyboard");
        assert!(
            g.categories
                .iter()
                .any(|i| i == "graphics.terminal-graphics"),
            "cats: {:?}",
            g.categories
        );
        assert!(g.categories.iter().any(|i| i == "multimedia.image-editors"));
        assert!(g.keywords.iter().any(|i| i == "draw"));
        assert!(g.keywords.iter().all(|k| !STOPWORDS.contains(&k.as_str())));
        assert!(!g.keywords.iter().any(|k| k == "software" || k == "that"));
    }

    #[test]
    fn goal_parsing_video_and_password() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        let g = parse_goal("i want to edit video");
        assert!(g.categories.iter().any(|i| i == "multimedia.video-editors"));
        assert!(g.synonyms_applied.iter().any(|i| i == "kdenlive"));
        let g2 = parse_goal("manage my passwords securely");
        assert!(
            g2.categories
                .iter()
                .any(|i| i == "security.password-management")
        );
        assert!(g2.synonyms_applied.iter().any(|i| i == "keepass"));
    }

    #[test]
    fn classify_perf_is_fast() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        // Warm the cache (first call builds the AC automaton)
        let _ = classify_pkg("ripgrep", "Regexp search tool");
        // Benchmark: 1000 classifications in debug mode must take < 100ms
        // (= < 100µs/pkg). The old version was 575µs release / 2258µs debug;
        // the Aho-Corasick version is ~1-5µs release / ~50µs debug.
        let start = std::time::Instant::now();
        for i in 0..1000 {
            let _ = classify_pkg(
                &format!("test-package-{i}"),
                "A test package description with some keywords like http server proxy",
            );
        }
        let elapsed = start.elapsed();
        let per_pkg_us = elapsed.as_micros() / 1000;
        assert!(
            per_pkg_us < 100,
            "classify_pkg too slow: {per_pkg_us}µs/pkg (target <100µs debug, old was 575µs release / 2258µs debug) — {elapsed:?} for 1000 calls"
        );
    }

    #[test]
    fn classification_examples() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            classify_pkg("ripgrep", "Regexp search tool"),
            "cli.file-search"
        );
        assert_eq!(
            classify_pkg("mpv", "a media player"),
            "multimedia.video-players"
        );
        assert_eq!(classify_pkg("htop", "process monitor"), "system.monitoring");
        assert_eq!(
            classify_pkg("gnupg", "gnu privacy guard crypto"),
            "security.cryptography"
        );
        assert_eq!(
            classify_pkg("ttf-fira-code", "monospaced font"),
            "fonts.monospace"
        );
        assert_eq!(classify_pkg("python-numpy", ""), "science.math-numeric");
        assert_eq!(
            classify_pkg("zzz-unknown", "nothing matches here really"),
            ""
        );
    }

    #[test]
    fn tree_shape() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        assert!(tops().len() >= 12, "tops: {:?}", tops());
        let sys = subs("system").unwrap();
        assert!(sys.iter().any(|s| s == "shells"), "system subs: {sys:?}");
        assert!(subs("nope").is_none());
        // every top has leaves
        let leaves = effective_leaves();
        assert!(leaves.len() >= 150, "leaves: {}", leaves.len());
    }

    #[test]
    fn external_taxonomy_import() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        let toml_data = r#"
[[leaf]]
top = "custom-top"
sub = "custom-sub"
keywords = ["flurbwidge"]
"#;
        let n = import_toml(toml_data).unwrap();
        assert_eq!(n, 1);
        assert!(tops().iter().any(|i| i == "custom-top"));
        assert_eq!(classify_pkg("flurbwidge-tool", ""), "custom-top.custom-sub");
        // reset to built-in for other tests
        *EXTERNAL.leaves.lock().unwrap() = None;
        invalidate_cache();
        assert!(tops().iter().any(|i| i == "development"));
    }

    /// An invalid `taxonomy.toml` was ignored without a word. It is still
    /// not used, but the reason and the file are said.
    #[test]
    fn an_unusable_taxonomy_file_is_reported_with_its_name_and_reason() {
        let _serial = TAXONOMY.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("yapacc-taxonomy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("taxonomy.toml");
        let before = effective_leaves();
        for (bad, reason) in [
            ("[[leaf]\ntop = ", "parse error"),
            ("leaf = 3\n", "parse error"),
            (
                "[[leaf]]\ntop = \"a\"\nsub = \"b\"\nkeywords = []\n",
                "lacks top/sub/keywords",
            ),
        ] {
            std::fs::write(&file, bad).unwrap();
            let e = load_external(&file).unwrap_err();
            assert!(e.contains(reason), "{bad:?}: {e}");
            let w = external_warning(&file, &e);
            assert!(w.starts_with("warning: "), "{w}");
            assert!(w.contains(&file.display().to_string()), "{w}");
            assert!(w.contains("the built-in categories apply"), "{w}");
        }
        let e = load_external(&dir.join("missing.toml")).unwrap_err();
        assert!(e.starts_with("cannot be read"), "{e}");
        // nothing of it was installed
        assert_eq!(effective_leaves(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
