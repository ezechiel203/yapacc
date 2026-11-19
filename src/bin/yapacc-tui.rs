// yapacc-tui — interactive terminal interface (optional component)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
// Every action this interface performs goes through the `yapacc` core
// binary, so gates, plans and the ledger behave exactly as on the CLI.
// `--config FILE` is loaded here and forwarded to every core run.
//
// Exit codes: 2 for a bad argument or configuration, 6 when there is no
// terminal to draw on.

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
                    "yapacc-tui — interactive interface for yapacc\n\nUsage: yapacc-tui [--config <FILE>]\n\nNeeds a terminal. Press ? inside the interface for help."
                );
                return;
            }
            "-V" | "--version" => {
                println!("yapacc-tui {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                eprintln!("error: unexpected argument '{other}' (see --help)");
                std::process::exit(2);
            }
        }
    }
    let res = yapacc::config::init(config_path.as_deref())
        .map(|_| ())
        .map_err(yapacc::Error::usage)
        .and_then(|()| yapacc::tui::run());
    std::process::exit(yapacc::error::report(res));
}
