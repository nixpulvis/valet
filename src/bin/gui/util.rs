use eframe::egui;
use rand_core::{OsRng, RngCore};

pub fn button_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    (ui.fonts(|f| {
        labels
            .iter()
            .map(|s| {
                f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE)
                    .rect
                    .width()
            })
            .fold(0., f32::max)
    }) + ui.spacing().button_padding.x * 2.)
        .max(ui.spacing().interact_size.x)
        .ceil()
}

pub fn generate_password() -> String {
    const CHARSET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*";
    let mut rng = OsRng;
    let mut buf = [0u8; 1];
    let mut password = String::with_capacity(20);
    while password.len() < 20 {
        rng.fill_bytes(&mut buf);
        let idx = buf[0] as usize;
        // rejection sampling to avoid modulo bias
        if idx < 256 - (256 % CHARSET.len()) {
            password.push(CHARSET[idx % CHARSET.len()] as char);
        }
    }
    password
}
