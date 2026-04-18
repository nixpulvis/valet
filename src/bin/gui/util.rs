use eframe::egui;

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
