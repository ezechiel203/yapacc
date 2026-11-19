# Security policy

## Reporting a vulnerability

Please report security problems **privately**, not in a public issue.

Use GitHub's private vulnerability reporting for this repository:
<https://github.com/ezechiel203/yapacc/security/advisories/new>

Include the output of `yapacc --version`, `yapacc platform` and
`yapacc doctor`, the exact command, and what happened. A proof of concept
that runs in a container or a virtual machine is the most useful thing you
can send.

You will get an answer within seven days. yapacc is maintained by one
person: a fix for a confirmed problem is the priority, and you will be told
honestly how long it is expected to take. Reports are credited in the
changelog unless you ask otherwise.

## Supported versions

Only the latest release receives fixes. yapacc is at version 0.x: there are
no maintained older branches.

## What counts

yapacc runs your package manager with your privileges, so these are
treated as vulnerabilities:

- a change applied without the consent the [gate](docs/handbook/gate.md)
  promises, including through the MCP server in its default configuration;
- anything from a package name, a log file, a repository index or a network
  answer being executed or read as a command-line option;
- a file written or deleted outside the places yapacc says it writes to
  (see [safety and privacy](docs/handbook/safety.md#what-it-writes));
- an escape from the build sandbox to the invoking user's files;
- a verification that reports success for content it did not verify.

These are known limits and are documented, not vulnerabilities: packages
you agreed to install run their own scripts as root; old packages fetched
from Launchpad are not verified and snapshot.debian.org files are checked
by hash only, and the output says so; the build sandbox has no seccomp
filter.

The threat model, protection by protection, is in
[docs/SECURITY.md](docs/SECURITY.md).
