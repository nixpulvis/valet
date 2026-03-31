const MIN_SIZE: [f32; 2] = [200., 150.];
const UNLOCKED_DEFAULT_SIZE: [f32; 2] = [400., 600.];

fn main() {
    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_inner_size(MIN_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_resizable(false);
    eframe::run_native(
        "Valet",
        options,
        Box::new(|ctx| Ok(Box::new(App::new(ctx)))),
    )
    .expect("eframe run failed");
}

mod app;
use self::app::App;
mod util;
mod widget;
