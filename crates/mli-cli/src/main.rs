use std::path::PathBuf;

use anyhow::{Result, anyhow};

#[derive(Debug, Default)]
struct Cli {
    app_server_bin: Option<PathBuf>,
    line_mode: bool,
}

impl Cli {
    fn parse() -> Result<Self> {
        let mut cli = Self::default();
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            let raw = arg.to_string_lossy();
            if raw == "--help" || raw == "-h" {
                print_help();
                std::process::exit(0);
            }
            if raw == "--app-server-bin" {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--app-server-bin requires a path"))?;
                cli.app_server_bin = Some(PathBuf::from(value));
                continue;
            }
            if raw == "--line-mode" {
                cli.line_mode = true;
                continue;
            }
            if let Some(value) = raw.strip_prefix("--app-server-bin=") {
                cli.app_server_bin = Some(PathBuf::from(value));
                continue;
            }
            return Err(anyhow!("unknown argument `{}`", raw));
        }
        Ok(cli)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse()?;
    if cli.line_mode {
        mli_tui::run_line_mode_tui(cli.app_server_bin)
    } else {
        mli_tui::run_default_tui(cli.app_server_bin)
    }
}

fn print_help() {
    println!("ml-intern");
    println!("  Transcript-first ML engineering terminal app");
    println!();
    println!("Options:");
    println!(
        "  --app-server-bin <path>  Override the local app-server binary used by the TUI client."
    );
    println!(
        "  --line-mode              Run the legacy line-mode client instead of the full-screen TUI."
    );
    println!("  -h, --help               Show this help message.");
}
