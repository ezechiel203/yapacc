# The interactive interface

`yapacc i` opens a full-screen interface in the terminal. It is an optional
component, the separate program `yapacc-tui`; when it is not installed,
`yapacc i` says so and exits with code 6.

It shows the same reports as the command line and adds nothing of its own.
It is for exploring: reading the timeline, filtering the installed list,
looking at what `clean` found.

## The home screen

The home screen is a menu of questions: what changed recently, what the
next upgrade will do, what can be cleaned up, what works on this machine.
`Enter` opens the highlighted line, and each line also has its own letter.

## Keys

| Where | Key | Action |
|---|---|---|
| everywhere | `?` | help |
| | `Esc` or `q` | back; `q` on the home screen quits |
| | `Ctrl-C` | quit at once |
| | `j` `k`, arrows, `PgUp` `PgDn`, `g` `G` | move |
| reports | `/` | filter by text |
| | `r` | read again |
| | `Enter` | show the whole line when it is cut |
| | `u` | in the upgrade briefing: plan the upgrade |
| | `o` | in the clean-up report: plan the removal of unneeded packages |
| browsing | `Tab`, `1`–`6` | change tab |
| Installed | `/` filter, `Space` select, `i` why, `d` dependencies, `x` remove, `e` mark explicit | |
| Search | type and `Enter`; `a` install, `i` info, `A` include the AUR (Arch) | |
| External | `Enter` details, `x` remove | |
| Run | type any yapacc arguments; the output is shown, and nothing can prompt | |

## Changing the system from the interface

The interface never answers a confirmation for you.

Every change is first shown as a plan, which is a dry run: nothing has
happened yet. To carry it out you move to `[ Apply ]` and press `Enter`.
The interface then leaves the screen and runs the command in your terminal,
where yapacc itself shows the plan and asks, exactly as described in
[the gate](gate.md). The `sudo` or `doas` password prompt appears there as
well.

Run the interface as yourself. There is no reason to start it as root.

The Run tab shows output only. It does not run a command with `--yes` or
`--noconfirm`, because nothing typed there has been through the plan
screen. Without a terminal, `yapacc i` exits with code 6.
