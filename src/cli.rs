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
