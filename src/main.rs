mod app;
mod backend;
mod input;
mod macro_store;
mod mode;
mod render;

fn main() -> anyhow::Result<()> {
    let mut backend = backend::wayland::WaylandBackend::new()?;
    app::run(&mut backend)
}
