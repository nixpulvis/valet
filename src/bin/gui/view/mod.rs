use eframe::egui::Context;

pub trait View {
    fn show(&mut self, ctx: &Context);
}

pub mod primary;
pub use self::primary::{Locked, Unlocked};
