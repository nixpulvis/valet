use crate::util::button_width;
use eframe::egui;

pub struct PasswordInput<'a> {
    // TODO: Use a Password type
    text: &'a mut String,
    visible: &'a mut bool,
    reserved_right: f32,
}

impl<'a> PasswordInput<'a> {
    pub fn new(text: &'a mut String, visible: &'a mut bool) -> Self {
        Self {
            text,
            visible,
            reserved_right: 0.,
        }
    }

    pub fn reserved_right(mut self, width: f32) -> Self {
        self.reserved_right = width;
        self
    }
}

impl egui::Widget for PasswordInput<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let btn_width = button_width(ui, &["Show", "Hide"]);

            let reserved = btn_width
                + spacing * 2.
                + if self.reserved_right > 0. {
                    self.reserved_right + spacing
                } else {
                    0.
                };
            let text_width = (ui.available_width() - reserved).max(0.);
            let response = ui.add(
                egui::TextEdit::singleline(self.text)
                    .password(!*self.visible)
                    .desired_width(text_width),
            );
            let label = if *self.visible { "Hide" } else { "Show" };
            if ui
                .add(egui::Button::new(label).min_size(egui::vec2(btn_width, 0.)))
                .clicked()
            {
                *self.visible = !*self.visible;
            }
            response
        })
        .inner
    }
}
