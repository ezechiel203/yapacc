// yapacc-mcp — Model Context Protocol server over stdio (optional component)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
// Reads are answered by the `yapacc` core binary (or, for four of them, in
// this process). A tool that would change the machine only returns the plan
// and the command the human runs in their own terminal; it applies only
// when the owner allowed that in a root-owned configuration file. See the
// header of src/mcp.rs for the whole model.
//
// Exit code 2: a bad argument or a bad configuration.

fn main() {
    let mut config_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => match args.next() {
                Some(p) => config_path = Some(p),
                None => {
                    eprintln!("error: --config needs a file");
                    std::process::exit(2);
                }
            },
            "-h" | "--help" => {
                println!(
                    "yapacc-mcp — MCP stdio server for yapacc\n\nUsage: yapacc-mcp [--config <FILE>]\n\nSpeaks JSON-RPC 2.0 on stdin/stdout, one message per line.\nSee `yapacc docs ai`."
                );
                return;
            }
            "-V" | "--version" => {
                println!("yapacc-mcp {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                eprintln!("error: unexpected argument '{other}' (see --help)");
                std::process::exit(2);
            }
        }
    }
    // a configuration that cannot be used is a usage error (exit 2)
    let configured = config_path
        .as_deref()
        .map_or(Ok(()), yapacc::mcp::set_core_config)
        .and_then(|()| yapacc::config::init(config_path.as_deref()))
        .map(|_| ())
        .map_err(yapacc::Error::usage);
    let res = configured.and_then(|()| yapacc::mcp::serve().map_err(yapacc::Error::from));
    std::process::exit(yapacc::error::report(res));
}
