//! CLI subcommand dispatch. The binary without subcommands runs the web
//! server (normal service mode). With a subcommand, it runs that command
//! to completion and exits.

pub mod env_cmd;

use std::env;
use std::ffi::OsString;

/// Write one formatted line to `w`, ignoring write errors.
///
/// CLI output must be best-effort: the std print macros panic when the
/// underlying write fails, and the release profile's `panic = "abort"`
/// (immediate-abort on the Tier-3 mipsel build) turns that panic into a
/// `Trace/breakpoint trap`. The Rust runtime starts every process with
/// SIGPIPE ignored, so a reader that closes the pipe early
/// (`modem-interface env show | head -5`) makes each subsequent stdout
/// write fail with EPIPE — which used to abort the whole process
/// (hardware repro 2026-06-11, ZBT-WG3526 mipsel_24kc). Swallowing the
/// error lets the command finish quietly with its normal exit code; the
/// remaining output is simply discarded, which is the observable behavior
/// of well-behaved tools under `| head`.
///
/// The SIGPIPE disposition itself is deliberately NOT changed (e.g. to
/// SIG_DFL on the CLI path): std/tokio socket writes on Linux do not pass
/// MSG_NOSIGNAL and rely on SIGPIPE staying ignored, so a default
/// disposition would let a TCP write inside the `env set` health probe
/// kill the process mid-switch, skipping the FileSnapshot rollback.
/// Output-side tolerance is strictly safer, and it leaves the daemon's
/// signal handling untouched by construction — this function only ever
/// runs when a CLI print site calls it.
pub(crate) fn write_line_best_effort(w: &mut dyn std::io::Write, args: std::fmt::Arguments<'_>) {
    let _ = writeln!(w, "{args}");
}

/// stdout line output for CLI subcommands. Never panics on write failure
/// (see [`write_line_best_effort`]); byte-identical to the std macro when
/// the write succeeds.
macro_rules! cli_println {
    () => {
        $crate::cli::write_line_best_effort(&mut ::std::io::stdout(), format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::cli::write_line_best_effort(&mut ::std::io::stdout(), format_args!($($arg)*))
    };
}

/// stderr line output for CLI subcommands. Never panics on write failure
/// (see [`write_line_best_effort`]); byte-identical to the std macro when
/// the write succeeds.
macro_rules! cli_eprintln {
    () => {
        $crate::cli::write_line_best_effort(&mut ::std::io::stderr(), format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::cli::write_line_best_effort(&mut ::std::io::stderr(), format_args!($($arg)*))
    };
}

pub(crate) use {cli_eprintln, cli_println};

/// Parse argv and, if a subcommand is present, run it and return its
/// exit code. Returns `None` to indicate "no subcommand — caller should
/// start the server."
pub async fn dispatch() -> Option<u8> {
    let args: Vec<OsString> = env::args_os().collect();
    let sub = args.get(1).and_then(|s| s.to_str())?;

    match sub {
        "env" => {
            let code = env_cmd::run(&args[2..]).await;
            Some(code)
        }
        "--version" | "-V" => {
            cli_println!("modem-interface {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        "--help" | "-h" => {
            print_help();
            Some(0)
        }
        "--reset-password" => None,
        _ => {
            cli_eprintln!("modem-interface: unknown subcommand '{sub}'");
            cli_eprintln!("Run 'modem-interface --help' for usage.");
            Some(2)
        }
    }
}

fn print_help() {
    cli_println!("modem-interface — web UI and background services for cellular modems");
    cli_println!();
    cli_println!("USAGE:");
    cli_println!("  modem-interface                  start the web server (default)");
    cli_println!("  modem-interface env show         print current environment + URLs");
    cli_println!("  modem-interface env set <name>   switch to a known environment");
    cli_println!("  modem-interface --version        print version and exit");
    cli_println!();
    cli_println!("Known environments: production, staging");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// What stdout looks like after `| head` exits: every write fails with
    /// EPIPE/BrokenPipe (the runtime keeps SIGPIPE ignored, so the failure
    /// surfaces as an error return, not a signal).
    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn write_line_best_effort_does_not_panic_on_broken_pipe() {
        // The std print macros panic here; with panic=abort (release/mipsel)
        // that abort is the `Trace/breakpoint trap` this fix removes.
        let mut w = BrokenPipeWriter;
        write_line_best_effort(&mut w, format_args!("Environment:  {}", "staging"));
        write_line_best_effort(&mut w, format_args!(""));
    }

    /// A writer that fails part-way through, like a pipe buffer that fills
    /// exactly at the boundary before the reader goes away.
    struct FailAfterFirstWrite {
        wrote: bool,
        buf: Vec<u8>,
    }

    impl Write for FailAfterFirstWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.wrote {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            self.wrote = true;
            // Accept a single byte so writeln's retry loop must call again.
            self.buf.push(buf[0]);
            Ok(1)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_line_best_effort_does_not_panic_on_mid_line_broken_pipe() {
        let mut w = FailAfterFirstWrite { wrote: false, buf: Vec::new() };
        write_line_best_effort(&mut w, format_args!("Portal:       https://example.com"));
    }

    #[test]
    fn write_line_best_effort_output_is_byte_identical_to_std_macro_format() {
        // Requirement: normal (pipe-not-closed) operation keeps byte-identical
        // output — formatted text plus a single trailing newline.
        let mut buf: Vec<u8> = Vec::new();
        write_line_best_effort(
            &mut buf,
            format_args!("APK feed:     {}/{}", "https://packages.ctrl-modem.com/testing/apk", "mipsel_24kc"),
        );
        assert_eq!(
            buf,
            b"APK feed:     https://packages.ctrl-modem.com/testing/apk/mipsel_24kc\n"
        );

        let mut empty: Vec<u8> = Vec::new();
        write_line_best_effort(&mut empty, format_args!(""));
        assert_eq!(empty, b"\n");
    }

    /// Regression guard for "every CLI output path": all output in the CLI
    /// surface must go through the EPIPE-tolerant macros. A bare std print
    /// macro reintroduces the panic=abort trap under `| head` on mipsel.
    #[test]
    fn cli_sources_have_no_bare_std_print_macros() {
        let sources = [
            ("cli/mod.rs", include_str!("mod.rs")),
            ("cli/env_cmd.rs", include_str!("env_cmd.rs")),
        ];
        // Needle built by concatenation so this test's own source does not
        // contain it literally.
        let needle = String::from("print") + "ln!";
        for (name, src) in sources {
            let mut from = 0;
            while let Some(pos) = src[from..].find(&needle) {
                let abs = from + pos;
                let before = &src[..abs];
                // Allowed: cli_println! (prefix "cli_") and cli_eprintln!
                // (prefix "cli_e"). Anything else is a bare std print macro.
                assert!(
                    before.ends_with("cli_") || before.ends_with("cli_e"),
                    "{name}: bare std print macro at byte offset {abs} — use cli_println!/cli_eprintln! so an early-closed pipe cannot abort the CLI"
                );
                from = abs + needle.len();
            }
        }
    }
}
