use clap::Parser;

use crate::app::InitialMode;

#[derive(Parser)]
#[command(version, about = "Keyboard-driven mouse overlay")]
pub struct Args {
    /// Start in bisect mode (recursive grid subdivision)
    #[arg(long, group = "mode")]
    pub bisect: bool,

    /// Start in free mode (move the cursor manually)
    #[arg(long, group = "mode")]
    pub free: bool,

    /// Start in free mode at the center of the screen
    #[arg(long, group = "mode")]
    pub free_center: bool,

    /// Start in hint mode (clickable-element labels)
    #[arg(long, group = "mode")]
    pub hint: bool,

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

        if self.free {
            return InitialMode::Free;
        }

        if self.free_center {
            return InitialMode::FreeCenter;
        }

        if self.hint {
            return InitialMode::Hint;
        }

        InitialMode::Normal
    }
}
