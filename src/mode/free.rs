use crate::{
    backend::{Backend, KeyEvent},
    config::{config, Key},
    mode::{Mode, ModeTransition},
    render::render_free,
};

pub(super) fn handle_key<B: Backend>(
    width: u32,
    height: u32,
    key: &KeyEvent,
    backend: &mut B,
    x: u32,
    y: u32,
    speed: u32,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Close => Ok(ModeTransition::Exit),
        KeyEvent::Undo => Ok(ModeTransition::Back),
        KeyEvent::Click => {
            backend.click(x, y)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::DoubleClick => {
            backend.double_click(x, y)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::RightClick => {
            backend.right_click(x, y)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::MiddleClick => {
            backend.middle_click(x, y)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::ScrollUp => {
            backend.scroll_up()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollDown => {
            backend.scroll_down()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollLeft => {
            backend.scroll_left()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollRight => {
            backend.scroll_right()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::Char(ch) => handle_char(width, height, *ch, backend, x, y, speed),
        _ => Ok(ModeTransition::Stay),
    }
}

fn handle_char<B: Backend>(
    width: u32,
    height: u32,
    ch: char,
    backend: &mut B,
    x: u32,
    y: u32,
    speed: u32,
) -> anyhow::Result<ModeTransition> {
    let free = &config().free;
    let pressed = Key::Char(ch);
    let step = speed as i32;

    let (dx, dy) = if pressed == free.left {
        (-step, 0)
    } else if pressed == free.right {
        (step, 0)
    } else if pressed == free.up {
        (0, -step)
    } else if pressed == free.down {
        (0, step)
    } else {
        (0, 0)
    };

    if dx != 0 || dy != 0 {
        let max_x = width.saturating_sub(1) as i32;
        let max_y = height.saturating_sub(1) as i32;
        let new_x = (x as i32 + dx).clamp(0, max_x) as u32;
        let new_y = (y as i32 + dy).clamp(0, max_y) as u32;
        backend.move_mouse(new_x, new_y)?;
        return Ok(ModeTransition::Replace(Mode::Free {
            x: new_x,
            y: new_y,
            speed,
        }));
    }

    if pressed == free.fast {
        let new_speed = scale_speed(speed, free.fast_multiplier);
        return Ok(ModeTransition::Replace(Mode::Free {
            x,
            y,
            speed: new_speed,
        }));
    }

    if pressed == free.slow {
        let factor = if free.slow_multiplier > 0.0 {
            1.0 / free.slow_multiplier
        } else {
            1.0
        };
        let new_speed = scale_speed(speed, factor);
        return Ok(ModeTransition::Replace(Mode::Free {
            x,
            y,
            speed: new_speed,
        }));
    }

    Ok(ModeTransition::Stay)
}

fn scale_speed(speed: u32, factor: f32) -> u32 {
    let free = &config().free;
    let min = free.min_speed.max(1);
    let max = free.max_speed.max(min);
    let scaled = ((speed as f32) * factor).round() as i64;
    scaled.clamp(min as i64, max as i64) as u32
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    speed: u32,
) -> anyhow::Result<()> {
    render_free(pixels, width, height, x, y, speed);
    backend.present(pixels, width, height)
}
