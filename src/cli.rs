use clap::Parser;

use crate::app::InitialMode;

#[derive(Parser)]
#[command(version, about = "Keyboard-driven mouse overlay")]
pub struct Args {
    /// Start in bisect mode (recursive grid subdivision)
    #[arg(long, group = "command")]
    pub bisect: bool,

    /// Start in free mode (move the cursor manually)
    #[arg(long, group = "command")]
    pub free: bool,

    /// Start in free mode at the center of the screen
    #[arg(long, group = "command")]
    pub free_center: bool,

    /// Left-click at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub click: bool,

    /// Double-click at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub double_click: bool,

    /// Right-click at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub right_click: bool,

    /// Scroll up at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub scroll_up: bool,

    /// Scroll down at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub scroll_down: bool,

    /// Scroll left at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub scroll_left: bool,

    /// Scroll right at the current cursor position and exit (no overlay)
    #[arg(long, group = "command")]
    pub scroll_right: bool,

    /// Replay a saved macro by name or bind key, then exit (no overlay)
    #[arg(long = "macro", value_name = "NAME_OR_KEY", group = "command")]
    pub run_macro: Option<String>,

    /// List saved macros and exit
    #[arg(long, group = "command")]
    pub list_macros: bool,

    /// Allow multiple concurrent instances
    #[arg(long)]
    pub allow_multiple: bool,

    #[arg(
        long,
        group = "command",
        help = "Print the default config (TOML) to stdout and exit. Diff against your config.toml to see new options."
    )]
    pub print_default_config: bool,
}

/// A pointer action performed once at the current cursor position.
#[derive(Clone, Copy)]
pub enum Action {
    Click,
    DoubleClick,
    RightClick,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// What a single invocation of the binary should do. Exactly one is selected;
/// the `command` arg group makes the flags mutually exclusive.
pub enum Invocation {
    /// Open the overlay in the given mode (the default, daemon-less flow).
    Overlay(InitialMode),
    /// Synthesize a single pointer action at the cursor, no overlay.
    Action(Action),
    /// Replay a saved macro identified by name or bind key, no overlay.
    Macro(String),
    /// Print the saved macros to stdout.
    ListMacros,
    /// Print the default config to stdout.
    PrintConfig,
}

impl Args {
    pub fn invocation(&self) -> Invocation {
        if self.print_default_config {
            return Invocation::PrintConfig;
        }
        if self.list_macros {
            return Invocation::ListMacros;
        }
        if let Some(query) = &self.run_macro {
            return Invocation::Macro(query.clone());
        }
        if self.click {
            return Invocation::Action(Action::Click);
        }
        if self.double_click {
            return Invocation::Action(Action::DoubleClick);
        }
        if self.right_click {
            return Invocation::Action(Action::RightClick);
        }
        if self.scroll_up {
            return Invocation::Action(Action::ScrollUp);
        }
        if self.scroll_down {
            return Invocation::Action(Action::ScrollDown);
        }
        if self.scroll_left {
            return Invocation::Action(Action::ScrollLeft);
        }
        if self.scroll_right {
            return Invocation::Action(Action::ScrollRight);
        }
        if self.bisect {
            return Invocation::Overlay(InitialMode::Bisect);
        }
        if self.free {
            return Invocation::Overlay(InitialMode::Free);
        }
        if self.free_center {
            return Invocation::Overlay(InitialMode::FreeCenter);
        }
        Invocation::Overlay(InitialMode::Normal)
    }
}
