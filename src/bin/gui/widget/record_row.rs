use crate::util::button_width;
use eframe::egui;

pub struct RecordRow<'a> {
    label: &'a str,
    password: &'a str,
}

impl<'a> RecordRow<'a> {
    pub fn new(label: &'a str, password: &'a str) -> Self {
        Self { label, password }
    }
}

impl egui::Widget for RecordRow<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = ui.make_persistent_id(("record", self.label));
        let expanded_id = id.with("expanded");
        let show_pw_id = id.with("show_pw");
        let expanded = ui.data(|d| d.get_temp::<bool>(expanded_id).unwrap_or(false));
        let show_pw = ui.data(|d| d.get_temp::<bool>(show_pw_id).unwrap_or(false));

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let copy_width = button_width(ui, &["Copy"]);
                let spacing = ui.spacing().item_spacing.x;
                let label_width = (ui.available_width() - copy_width - spacing).max(0.);

                // allocate_space advances the cursor by exactly label_width.
                // new_child renders into that rect with an explicit left-to-right layout
                // without touching the cursor again, keeping the label left-aligned.
                let (_, label_rect) =
                    ui.allocate_space(egui::vec2(label_width, ui.spacing().interact_size.y));
                let resp = ui
                    .new_child(
                        egui::UiBuilder::new()
                            .max_rect(label_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    )
                    .add(
                        egui::Label::new(self.label)
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                if resp.clicked() {
                    ui.data_mut(|d| d.insert_temp(expanded_id, !expanded));
                    if expanded {
                        ui.data_mut(|d| d.insert_temp(show_pw_id, false));
                    }
                }
                if resp.hovered() {
                    ui.ctx()
                        .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }

                if ui
                    .add(egui::Button::new("Copy").min_size(egui::vec2(copy_width, 0.)))
                    .clicked()
                {
                    ui.ctx().copy_text(self.password.to_owned());
                }
            });

            if expanded {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 0,
                        right: 0,
                        top: 2,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let btn_width = button_width(ui, &["Show", "Hide"]);
                            let spacing = ui.spacing().item_spacing.x;
                            // With min_size on the button it renders at exactly btn_width,
                            // so one spacing gap is the correct reservation.
                            let text_width =
                                (ui.available_width() - btn_width - spacing * 2.).max(0.);

                            let mut pw = self.password.to_owned();
                            ui.add(
                                egui::TextEdit::singleline(&mut pw)
                                    .password(!show_pw)
                                    .interactive(false)
                                    .desired_width(text_width),
                            );

                            let toggle_label = if show_pw { "Hide" } else { "Show" };
                            if ui
                                .add(
                                    egui::Button::new(toggle_label)
                                        .min_size(egui::vec2(btn_width, 0.)),
                                )
                                .clicked()
                            {
                                ui.data_mut(|d| d.insert_temp(show_pw_id, !show_pw));
                            }
                        });
                    });
            }
        })
        .response
    }
}
