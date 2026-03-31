use crate::util::button_width;
use eframe::egui::{self, Id, Response, Ui};
use valet::password::Password;

pub struct PasswordInput<'a> {
    password: &'a mut Password,
    reserved_right: f32,
}

impl<'a> PasswordInput<'a> {
    pub fn new(password: &'a mut Password) -> Self {
        Self {
            password,
            reserved_right: 0.,
        }
    }

    pub fn reserved_right(mut self, width: f32) -> Self {
        self.reserved_right = width;
        self
    }
}

impl egui::Widget for PasswordInput<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        // TODO: Provide a way to set this if element order changes:
        // fn id_salt(self, id: Into<Id>))
        let id = Id::new(ui.next_auto_id());
        let mut state = State::load(ui, id);
        let response = ui
            .horizontal(|ui| {
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
                    egui::TextEdit::singleline(self.password)
                        .password(!state.visible)
                        .desired_width(text_width),
                );
                let label = if state.visible { "Hide" } else { "Show" };
                if ui
                    .add(egui::Button::new(label).min_size(egui::vec2(btn_width, 0.)))
                    .clicked()
                {
                    state.visible = !state.visible;
                }
                response
            })
            .inner;
        state.store(ui, id);
        response
    }
}

struct State {
    visible: bool,
}

impl State {
    fn load(ui: &Ui, id: Id) -> Self {
        State {
            visible: ui.data(|d| d.get_temp(id.with("visible")).unwrap_or_default()),
        }
    }

    fn store(self, ui: &Ui, id: Id) {
        ui.data_mut(|d| {
            d.insert_temp(id.with("visible"), self.visible);
        });
    }
}
