// yapacc — Yet Another PACkage Companion (core CLI)
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
// Built-in documentation: `yapacc docs` | manual: `yapacc docs --man`

/// Rust ignores SIGPIPE, so `yapacc undo | head` panicked on the first write
/// after the reader left. Restore the default (die quietly) like any CLI tool.
#[cfg(unix)]
fn restore_sigpipe() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: signal(2) with SIG_DFL, called once before any thread starts.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

fn main() {
    #[cfg(unix)]
    restore_sigpipe();
    std::process::exit(yapacc::error::report(yapacc::cli::run()));
}
