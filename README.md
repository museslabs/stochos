# stochos

> **stochos** (/'sto.xos/) — from Greek *στόχος*: aim, target, goal.

Keyboard-driven mouse control overlay for Wayland and X11. OSS alternative to [mouseless](https://mouseless.click).

![example](example.gif)

Displays a letter grid over your screen. Type a two-key combo to jump to a cell, refine with a sub-grid key, then act. Runs once per invocation (no daemon).

**Wayland:** Tested on **Hyprland**. Should work on any wlroots-based compositor with `zwlr_layer_shell_v1` and `zwlr_virtual_pointer_v1`.

**X11:** Tested on **i3**. Should work on any X11 window manager with the XTest extension.

## Install

With curl:

```sh
curl -fsSL https://raw.githubusercontent.com/museslabs/stochos/main/install.sh | sh
```

With cargo:

```sh
cargo install --git https://github.com/museslabs/stochos
```

From source:

```sh
git clone https://github.com/museslabs/stochos
cd stochos
cargo build --release                                          # both backends
cargo build --release --no-default-features --features wayland # Wayland only
cargo build --release --no-default-features --features x11     # X11 only
```

## Setup

### Hyprland

Bind it to a key in `hyprland.conf`:

```
bind = , SUPER_L, exec, stochos
```

### i3

Bind it to a key in `~/.config/i3/config`:

```
bindsym Super_L exec stochos
```

## Usage

1. Trigger the overlay
2. Type two letters to select a grid cell (e.g. `a` then `s`)
3. Type one more letter to refine within the sub-grid
4. Perform an action (see below)

### Default keys

| Key | Action |
|-----|--------|
| Space | Click |
| Enter | Double click |
| Delete | Right click |
| Escape | Close overlay |
| Backspace | Undo last step |
| Arrow keys | Scroll (up/down/left/right) |
| `/` | Start drag (select end point, then Space) |
| `` ` `` | Toggle macro recording |
| `@` | Replay macro by bind key |
| Tab | Search macros / quick-save position |

All keys are configurable (see Configuration below).

### Macros

Record multi-step mouse sequences for replay.

**Record:** Press `` ` `` to start recording. Navigate and act normally (Space to click, Enter to double-click, Tab to hover-only, `/` to drag). Press `` ` `` again to stop. You'll be prompted for an optional bind key and a name.

**Replay:** Press `@` then the bind key. Or press Tab to search by name, then Enter to select.

Macros are resolution-independent and stored at `~/.config/stochos/macros.json`.

## Configuration

Config file location: `~/.config/stochos/config.toml` (respects `XDG_CONFIG_HOME`).

All fields are optional. Missing fields use defaults. See `config.example.toml` for a detailed example.

```toml
[grid]
hints = ["a", "s", "d", "f", "j", "k", "l", ";", "g", "h", "q", "w", "e", "r", "t", "y", "u", "i", "o", "p"]
sub_hints = ["a", "s", "d", "f", "j", "k", "l", ";", "g", "h", "q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "z", "x", "c", "v", "b"]
sub_cols = 5
dynamic_grid = false
target_cell_size = 80

[keys]
click = "space"
double_click = "enter"
close = "escape"
undo = "backspace"
right_click = "delete"
scroll_up = "up"
scroll_down = "down"
scroll_left = "left"
scroll_right = "right"
macro_menu = "tab"
macro_record = "`"

[appearance]
cell_opacity = 102
highlight_opacity = 170
drag_opacity = 136
sub_cell_opacity = 170
```

### Grid

- `hints` sets the characters for the main grid. Grid size is `len(hints) x len(hints)` (default 20x20 = 400 cells).
- `sub_hints` sets the characters for the sub-grid. Sub-grid size is `sub_cols x (len(sub_hints) / sub_cols)` (default 5x5 = 25 cells).
- `sub_cols` sets how many columns the sub-grid has.
- `dynamic_grid` enables adaptive grid sizing based on screen dimensions (default: false).
- `target_cell_size` sets target cell size in pixels when `dynamic_grid` is enabled (default: 80). Try 60-120 depending on screen size.

### Appearance

Control the visual appearance of the overlay:

- `cell_opacity` — opacity for normal grid cells (0-255, default: 102)
- `highlight_opacity` — opacity for selected/highlighted cells (0-255, default: 170)
- `drag_opacity` — opacity during drag selection (0-255, default: 136)
- `sub_cell_opacity` — opacity for sub-grid cells (0-255, default: 170)

Lower values make the overlay more transparent, higher values make it more visible.

### Keys

Any keyboard key can be bound to any action.

Character keys use the character itself: `"a"`, `";"`, `"/"`, `"@"`, `"!"`.

Special keys use snake_case names:

| Name | Key |
|------|-----|
| `space` | Space |
| `enter` | Enter |
| `escape` | Escape |
| `backspace` | Backspace |
| `tab` | Tab |
| `delete` | Delete |
| `insert` | Insert |
| `home` | Home |
| `end` | End |
| `page_up` | Page Up |
| `page_down` | Page Down |
| `up` `down` `left` `right` | Arrow keys |
| `f1` through `f12` | Function keys |
| `caps_lock` `num_lock` `scroll_lock` | Lock keys |
| `print_screen` `pause` `context_menu` | System keys |
| `num_pad_0` through `num_pad_9` | Numpad digits |
| `num_pad_add` `num_pad_subtract` `num_pad_multiply` `num_pad_divide` `num_pad_decimal` `num_pad_enter` | Numpad operators |

Shifted characters work too: `"@"`, `"!"`, `"~"`, `"A"` (uppercase), etc.
