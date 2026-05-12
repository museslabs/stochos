use clap::Parser;

use crate::app::InitialMode;

#[derive(Parser)]
#[command(version, about = "Keyboard-driven mouse overlay")]
pub struct Args {
    /// Start in bisect mode (recursive grid subdivision)
    #[arg(long, group = "mode")]
    pub bisect: bool,

    /// Allow multiple concurrent instances
    #[arg(long)]
    pub allow_multiple: bool,
    #[arg(
        long,
        help = "Print the default config (TOML) to stdout and exit. Diff against your config.toml to see new options."
    )]
    pub print_default_config: bool,
}

impl Args {
    pub fn initial_mode(&self) -> InitialMode {
        if self.bisect {
            InitialMode::Bisect
        } else {
            InitialMode::Normal
        }
    }
}
