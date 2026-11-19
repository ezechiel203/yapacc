# AI assistants (MCP)

`yapacc mcp` runs a server that speaks the Model Context Protocol over
standard input and output. An AI assistant that supports MCP can connect to
it and use yapacc as a set of tools: ask why a package is installed, read
the timeline, get the upgrade briefing, and propose a change.

It is an optional component, the separate program `yapacc-mcp`. yapacc
contains no AI and holds no API key. The assistant, the model and the
billing are whatever you already use; yapacc is only the tool it calls.

## What an assistant can and cannot do

By default an assistant can **read** and **propose**. It cannot apply.

When it asks for an install, a removal, an upgrade or an undo, the server
returns the plan and the exact command for *you* to run in your own
terminal, where [the gate](gate.md) asks you. The tool's answer tells the
assistant not to claim that anything was changed.

This does not depend on the assistant behaving well. The tools accept a
`confirm` argument, and an assistant can set it to true and say you
approved. It makes no difference: unless the machine's administrator has
said otherwise, the server never applies.

### Letting an assistant apply

Two things are needed, and both must be true:

1. `apply = "allow"` under `[mcp]` in a configuration file that is **owned
   by root and not writable by anyone else**, passed to the server with
   `--config`;
2. the assistant sets `confirm` to true on the call.

```toml
# /etc/yapacc/mcp.toml, owned by root, mode 0644
[mcp]
# plan   propose only (default)
# allow  the assistant may apply a change when it states that you approved
apply = "allow"
```

The file has to be root's because your own configuration file is something
an assistant with access to your files could edit. `apply = "allow"` in a
file you own is ignored: the server prints a warning when it starts and
stays in plan mode.

> [!WARNING]
> `apply = "allow"` means you trust the assistant's statement that you
> approved. Privileges still apply: a change needs `sudo` or `doas`, and
> with no terminal those are called in non-interactive mode, so it works
> only where passwordless privileges exist for that command. Think before
> combining the two. Nothing records that a change was requested by an
> assistant: the native log shows it like any other.

## Connecting it

Point your assistant's MCP configuration at the command `yapacc mcp`. For
clients that use a JSON configuration:

```json
{
  "mcpServers": {
    "yapacc": { "command": "yapacc", "args": ["mcp"] }
  }
}
```

The server speaks protocol revision 2024-11-05.

## The tools

Read-only tools, which change nothing:

| Tool | What it returns |
|---|---|
| `yapacc_platform` | the kind of machine: distribution, package manager, init, snapshots |
| `yapacc_doctor` | what works on this machine |
| `yapacc_why` | why a package is installed |
| `yapacc_timeline` | package changes, newest first |
| `yapacc_briefing` | what the next upgrade will do |
| `yapacc_check` | pending updates |
| `yapacc_clean_report` | what can be reclaimed |
| `yapacc_restart_report` | what must be restarted after an upgrade |
| `yapacc_pending_configs` | configuration files left unmerged |
| `yapacc_snapshots` | existing filesystem snapshots |
| `yapacc_search`, `yapacc_resolve`, `yapacc_plan` | find a package, choose a source, show the install plan |
| `yapacc_names` | what a package is called on other distributions |
| `yapacc_ledger`, `yapacc_capabilities`, `yapacc_news` | installs outside the package manager; host capabilities; Arch news |

Staged tools. The answer of each begins with one of these lines, so that
neither you nor the assistant has to guess what happened:

| The answer begins with | Meaning |
|---|---|
| `PLAN ONLY — nothing was changed.` | the plan, and the command for you to run in your own terminal |
| `PLAN ONLY (confirm=false) — nothing was changed.` | applying is allowed, and the call did not say `confirm: true` |
| `PLAN FAILED — nothing was changed.` | the plan could not be made: a package no source has, a package to remove that is not installed, a failed simulation |
| `APPLIED: …` | the change was made; only when applying is allowed |
| `FAILED: …` | the change was tried and failed |

| Tool | Proposes |
|---|---|
| `yapacc_install` | installing a package |
| `yapacc_remove` | removing native packages |
| `yapacc_upgrade` | a full system upgrade |
| `yapacc_undo_last` | taking the last transaction back |

## What reaches the network

The server itself opens no socket and listens on nothing. Some tools make
yapacc contact a server, exactly as the matching command does: see
[safety and privacy](safety.md#what-leaves-the-machine). `yapacc_names`
sends the package name to repology.org, and its description tells the
assistant so.
