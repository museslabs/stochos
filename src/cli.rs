use clap::Parser;

use crate::app::InitialMode;
use crate::config::config;

#[derive(Parser)]
#[command(version, about = "Keyboard-driven mouse overlay")]
pub struct Args {
    /// Start in bisect mode (recursive grid subdivision)
    #[arg(long, group = "mode")]
    pub bisect: bool,

    /// Start in free mode (move the cursor manually)
    #[arg(long, group = "mode")]
    pub free: bool,

    /// Start in free mode at the current cursor position.
    /// Implies --free; overrides the `free.start_at_cursor` config setting.
    #[arg(long, conflicts_with = "bisect")]
    pub free_at_cursor: bool,

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
            return InitialMode::Bisect;
        }

        if self.free || self.free_at_cursor {
            let at_cursor = self.free_at_cursor || config().free.start_at_cursor;
            return InitialMode::Free { at_cursor };
        }

        InitialMode::Normal
    }
}
