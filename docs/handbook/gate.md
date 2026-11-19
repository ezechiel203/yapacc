# The gate

Every command that changes the system goes through the same gate. This page
describes it once; it applies to `install`, `remove`, `upgrade`, `refresh`,
`undo` and `snapshot create` on every package manager.

## What happens, in order

1. **Plan.** yapacc prints what will be installed, removed, upgraded or
   restored. A request that cannot be right stops here: removing a package
   that is not installed is "not found" (exit code 4) before anything is
   simulated.
2. **Simulation.** It runs the native manager's own dry run of that plan and
   prints the result as the manager says it. If the simulation fails, the
   change is refused (exit code 3) and you are not asked.
3. **Command.** It prints the exact command it is about to run.
4. **Gate.** It asks, unless you have already answered with a flag.
5. **Apply.** It runs the command, through `sudo` or `doas` when you are
   not root. Your yes is the only question: the native manager is told not
   to ask again (`apt-get -y`, `pacman --noconfirm`, `dnf -y`), and the
   plan shows the command with those flags.
6. **Record.** The native manager writes its log as usual. yapacc keeps no
   second record of native changes.
7. **Closing line.** yapacc says what the run amounted to, from the
   installed packages and not from the exit status alone: `Done`, or
   "Nothing was changed: apt had nothing to do (the installed packages are
   the same as before)", or, after a failure, whether packages were
   changed before it failed.

```console
$ yapacc install repo figlet
Plan: install figlet (through apt)

apt's own simulation (apt-get -s -y install -- figlet):
…
The following NEW packages will be installed:
  figlet
0 upgraded, 1 newly installed, 0 to remove and 24 not upgraded.
Inst figlet (2.2.5-3+b2 Debian:13.7/stable [amd64])
Conf figlet (2.2.5-3+b2 Debian:13.7/stable [amd64])

Will run:
  sudo apt-get install -y -- figlet
Proceed [y/N]? y
…
Setting up figlet (2.2.5-3+b2) ...
update-alternatives: using /usr/bin/figlet-figlet to provide /usr/bin/figlet (figlet) in auto mode
Processing triggers for man-db (2.13.1-1) ...

Done. `yapacc timeline` shows the change; `yapacc undo` can take it back.
```

Answer anything but yes and nothing happens:

```console
$ yapacc install repo figlet
Plan: install figlet (through apt)

…
Will run:
  sudo apt-get install -y -- figlet
Proceed [y/N]? n
error: Aborted: nothing was changed.
```

## How you answer

| You run | What happens |
|---|---|
| the plain command | the plan is shown and you are asked `[y/N]`; Enter means no |
| with `--yes` | the plan is shown and applied without a question |
| with `--dryrun` | the plan and the simulation are shown; nothing is applied |
| with `--script` | never prompts; needs `--yes` or `--dryrun`, otherwise it is a refusal |
| with input from a pipe | cannot answer the question, so it is a refusal unless `--yes` or `--dryrun` is given |

`--noconfirm` is accepted as another spelling of `--yes`.

The question follows your locale: `[o/N]` in French, `[j/N]` in German.
English `y` and `yes` are always accepted.

## A refusal

When the answer is no, or nobody could be asked, the command exits with
code **3** and has changed nothing. The message says which of the two it
was:

```console
$ yapacc --script upgrade
Plan: upgrade the system (through apt)
…
error: Proceed: --script never prompts; pass --yes to apply or --dryrun to plan only
$ echo $?
3
```

Code 3 is only ever a refusal. A change that was attempted and failed
exits 1. See [exit codes](scripting.md#exit-codes).

## A few questions `--yes` does not answer

`--yes` consents to the plan you could have read. It does not consent to
things that need your eyes or that yapacc cannot know. In those cases it
asks at the terminal and, when there is no terminal, refuses:

- packages with no earlier record when undoing a whole upgrade on zypper
  ([undo](undo.md#guards));
- the review of a build recipe (a PKGBUILD from the AUR) before it is
  built; `--no-review` is the explicit way to skip it;
- a non-free licence on a package installed from outside the
  repositories; `--non-free` accepts it in advance.

`--yes` and `--dryrun` together are a usage error (exit code 2).

## Privileges

Run yapacc as yourself. For the one command that needs root it uses `sudo`,
or `doas` where that is what the machine has, and the password prompt you
see is theirs.

- With `--script`, or when there is no terminal, the tool is called in its
  non-interactive mode (`sudo -n`, `doas -n`). If it would need a password,
  the command fails at once instead of waiting for input nobody can give.
- dnf and zypper need root even to simulate. At a terminal the simulation
  runs through `sudo` or `doas`, so the password prompt can come before
  the plan. In a run that may not ask (`--script`, no terminal) it runs
  behind `sudo -n` or `doas -n` when that lets you through; when it does
  not, the plan says "no simulation" and why.
- As root, no tool is used.
- When neither `sudo` nor `doas` is present and you are not root, the plan
  is still shown, and the command exits with code 6 naming what is missing.

## A snapshot first

When `[safety] snapshot` is set to `auto` or `auto-best-effort` in the
[configuration](configuration.md), yapacc takes a filesystem snapshot after
you consent and before the native command runs. The plan says so, and the
question becomes "Proceed (a snapper snapshot is taken first)". A dry run
takes no snapshot. When no snapshot can be taken, the plan says that too,
before you answer. See [snapshots](snapshots.md).

## What the gate is not

It is not a sandbox. Once you answer yes, the native manager runs with the
privileges it always has, and package scripts do what package scripts do.
The gate makes sure you saw the plan first.
