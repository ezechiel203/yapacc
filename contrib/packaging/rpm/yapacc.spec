# yapacc — Fedora-style spec: the portable core (no libalpm) and two
# optional components as subpackages.
#
# Layout (contrib/packaging/README.md):
#   %%{_bindir}/yapacc                    core
#   %%{_libexecdir}/yapacc/yapacc-tui     yapacc-tui subpackage (`yapacc i`)
#   %%{_libexecdir}/yapacc/yapacc-mcp     yapacc-mcp subpackage (`yapacc mcp`)
# The core looks for its components in ../libexec/yapacc relative to
# itself, so /usr/libexec/yapacc is found from /usr/bin/yapacc.

# The only place this file names the project's address; URL and Source0
# derive from it. Source0 is the release asset of that name (git archive of
# the tag), not GitHub's generated archive.
%global project_url https://github.com/ezechiel203/yapacc

# Tests run by default; `rpmbuild --without check` skips them.
%bcond_without check

Name:           yapacc
Version:        0.1.0
Release:        1%{?dist}
Summary:        Optional companion to the native package manager

# The binaries also link, statically, the crates pinned in Cargo.lock
# (MIT, Apache-2.0 and similar). List them here before submitting to
# Fedora proper.
License:        GPL-2.0-only
URL:            %{project_url}
Source0:        %{project_url}/releases/download/v%{version}/%{name}-%{version}.tar.gz

# This spec downloads the crates pinned in Cargo.lock while building
# (COPR: enable network access for the build). A Fedora-proper package
# would use rust-packaging macros with vendored or packaged crates instead;
# this spec calls cargo directly, so the distribution's build flags
# (%%cargo_build) are not applied.
BuildRequires:  cargo >= 1.89
BuildRequires:  rust >= 1.89
BuildRequires:  gcc

Suggests:       %{name}-tui
Suggests:       %{name}-mcp
Suggests:       flatpak
Suggests:       curl

%description
yapacc (Yet Another PACkage Companion) works beside dnf without replacing
it. It tells where a package should come from, why something is installed
and where it came from, and it gates and records the changes made through
it.

It keeps no copy of what the native package manager knows, installs no
daemon, timer, hook or alias, and its read commands change nothing.

This package contains the core command line.

%package        tui
Summary:        Interactive interface for yapacc
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description    tui
The interactive terminal interface started by "yapacc i". Every action it
offers is run by the core.

%package        mcp
Summary:        MCP server for yapacc
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description    mcp
The Model Context Protocol server started by "yapacc mcp". An AI assistant
connected through it can read and can propose; it cannot apply a change
unless that is enabled in the configuration.

%prep
%autosetup -n %{name}-%{version}

%build
# The core and the MCP server carry no optional feature.
cargo build --release --locked --no-default-features --bin yapacc --bin yapacc-mcp
# Only the interactive interface needs the tui feature.
cargo build --release --locked --no-default-features --features tui --bin yapacc-tui

mkdir -p completions
target/release/yapacc completions bash > completions/yapacc
target/release/yapacc completions zsh > completions/_yapacc
target/release/yapacc completions fish > completions/yapacc.fish

%install
install -Dpm 0755 target/release/yapacc %{buildroot}%{_bindir}/yapacc
install -Dpm 0755 target/release/yapacc-tui %{buildroot}%{_libexecdir}/yapacc/yapacc-tui
install -Dpm 0755 target/release/yapacc-mcp %{buildroot}%{_libexecdir}/yapacc/yapacc-mcp
install -Dpm 0644 contrib/man/yapacc.1 %{buildroot}%{_mandir}/man1/yapacc.1
install -Dpm 0644 completions/yapacc %{buildroot}%{_datadir}/bash-completion/completions/yapacc
install -Dpm 0644 completions/_yapacc %{buildroot}%{_datadir}/zsh/site-functions/_yapacc
install -Dpm 0644 completions/yapacc.fish %{buildroot}%{_datadir}/fish/vendor_completions.d/yapacc.fish

%if %{with check}
%check
# Some tests reach for the network; they are written to tolerate its
# absence. In a build root where they still fail: --without check.
#
# %%check runs after %%install, so the binaries this step rebuilds with the
# tui feature are not the ones that were installed.
cargo test --release --locked --no-default-features --features tui
%endif

%files
%license LICENSE
%doc README.md
%{_bindir}/yapacc
%dir %{_libexecdir}/yapacc
%{_mandir}/man1/yapacc.1*
%{_datadir}/bash-completion/completions/yapacc
%{_datadir}/zsh/site-functions/_yapacc
%{_datadir}/fish/vendor_completions.d/yapacc.fish

%files tui
%{_libexecdir}/yapacc/yapacc-tui

%files mcp
%{_libexecdir}/yapacc/yapacc-mcp

%changelog
* Mon Nov 16 2026 Alexandre BETRY <ezechiel203@users.noreply.github.com> - 0.1.0-1
- Initial packaging: yapacc (core), yapacc-tui and yapacc-mcp
