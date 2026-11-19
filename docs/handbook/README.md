# The yapacc handbook

yapacc is an optional companion to the package manager you already have. It
answers four questions that every system eventually asks, and it can make
changes through your package manager without taking it over.

| The question | The command |
|---|---|
| Why is this package here? | [`yapacc why NAME`](why.md) |
| What changed since the machine last worked? | [`yapacc timeline --since-boot`](timeline.md) |
| What will the next upgrade do? | [`yapacc briefing`](briefing.md) |
| Can I take the last change back? | [`yapacc undo`](undo.md) |

It works beside `pacman`, `apt`, `dnf`, `zypper`, `apk`, `xbps` and FreeBSD's
`pkg`. You keep using them. yapacc reads what they already record and, when
you ask it to change something, runs the command they would run, after
showing it to you.

## How to read this handbook

- New here: [install](install.md) it, then take the
  [ten-minute tour](quick-start.md). Every command in the tour is read-only.
- Before you let it change anything, read [the gate](gate.md). It is one
  page and it explains every prompt you will see.
- Looking for one command: [every command](commands.md) lists them all,
  including the specialist ones that `yapacc --help` leaves out.
- Wondering whether it works on your system:
  [what works where](support.md) says what was run, on which system, and
  what was not.

The same text ships inside the program. `yapacc docs` lists the built-in
topics and `yapacc docs --man | man -l -` shows the manual page, so the
documentation is there when the network is not.

## The rules yapacc keeps

These are promises about behaviour, and each one can be checked against the
code. They decide what yapacc will never do.

1. **Your package manager stays yours.** yapacc keeps no copy of what the
   native manager knows. Install or remove anything directly and yapacc is
   never confused by it.
2. **Nothing runs unless you ask.** No daemon, timer, hook or shell alias is
   installed by default.
3. **Read commands change nothing**, and work on a machine that has never
   used yapacc before.
4. **Removable without trace.** Delete yapacc and everything it installed
   keeps working, and can be removed with native tools.
5. **Every change is gated.** A plain command asks. `--yes` consents.
   `--dryrun` only plans. A refusal exits with code 3 and changes nothing.
6. **Changes go through your package manager**, as the command it would run
   itself, shown in full, after its own simulation. The native log is the
   record; `timeline` and `undo` read it.
7. **A protection says what it does not protect against**, in the output
   and not only in the manual.

One read command talks to the outside: [`names`](commands.md#names) sends
the package name you give it to repology.org, and says so. Everything a
command may contact is listed under
[safety and privacy](safety.md#what-leaves-the-machine).

## What yapacc is not

- **Not a package manager.** It has no repository and no package format.
- **Not a backup.** `undo` restores package versions. It does not restore
  configuration files or data; a [filesystem snapshot](snapshots.md) does.
- **Not a service.** Nothing stays running and nothing contacts a server
  on its own.
- **Not a configuration system.** It does not manage dotfiles, services or
  a fleet of machines.

## The three programs

| Program | What it is | Needed |
|---|---|---|
| `yapacc` | the command line; everything in this handbook | yes |
| `yapacc-tui` | [an interactive interface](tui.md), started with `yapacc i` | no |
| `yapacc-mcp` | [a server for AI assistants](mcp.md), started with `yapacc mcp` | no |

The core is one static file of under 6 MB with no shared library. When a
command needs a tool the machine lacks (`curl` for the news feed, `sudo` or
`doas` for a change), it names the tool and exits with code 6.
