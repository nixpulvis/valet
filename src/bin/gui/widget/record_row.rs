use crate::util::button_width;
use eframe::egui;
use egui_inbox::UiInbox;
use std::sync::Arc;
use tokio::runtime::Runtime;
use valet::{Lot, Record, db::Database, password::Password, record::Label, uuid::Uuid};

enum PasswordEvent {
    Copy(Password),
    Show(Password),
}

pub struct RecordRow<'a> {
    label: &'a Label,
    record_uuid: &'a Uuid<Record>,
    lot: Arc<Lot>,
    db: &'a Arc<Database>,
    rt: &'a Runtime,
}

impl<'a> RecordRow<'a> {
    pub fn new(
        label: &'a Label,
        record_uuid: &'a Uuid<Record>,
        lot: Arc<Lot>,
        db: &'a Arc<Database>,
        rt: &'a Runtime,
    ) -> Self {
        Self {
            label,
            record_uuid,
            lot,
            db,
            rt,
        }
    }
}

impl egui::Widget for RecordRow<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = ui.make_persistent_id(("record", self.record_uuid.to_string()));
        let expanded_id = id.with("expanded");
        let shown_pw_id = id.with("shown_pw");
        let pw_inbox_id = id.with("pw_inbox");

        let expanded = ui.data(|d| d.get_temp::<bool>(expanded_id).unwrap_or(false));

        let pw_inbox: Arc<UiInbox<PasswordEvent>> =
            ui.data_mut(|d| d.get_temp(pw_inbox_id).unwrap_or_default());

        for event in pw_inbox.read(ui.ctx()) {
            match event {
                PasswordEvent::Copy(pw) => ui.ctx().copy_text(pw.to_string()),
                // TODO: The revealed Password sits in egui's temp data until
                // the row collapses or egui evicts it. Zeroizes on drop, but
                // we should auto-evict after an idle window.
                PasswordEvent::Show(pw) => ui.data_mut(|d| d.insert_temp(shown_pw_id, pw)),
            }
        }

        ui.data_mut(|d| d.insert_temp(pw_inbox_id, pw_inbox.clone()));

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
                        egui::Label::new(self.label.to_string())
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                if resp.clicked() {
                    ui.data_mut(|d| d.insert_temp(expanded_id, !expanded));
                    if expanded {
                        ui.data_mut(|d| d.remove::<Password>(shown_pw_id));
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
                    spawn_fetch(
                        self.rt,
                        self.db.clone(),
                        self.lot.clone(),
                        self.record_uuid.clone(),
                        pw_inbox.sender(),
                        PasswordEvent::Copy,
                    );
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

                            let shown_pw = ui.data(|d| d.get_temp::<Password>(shown_pw_id));
                            let is_shown = shown_pw.is_some();
                            let mut pw = shown_pw
                                .unwrap_or_else(|| "xxxxxxxx".try_into().unwrap());
                            ui.add(
                                egui::TextEdit::singleline(&mut pw)
                                    .password(!is_shown)
                                    .interactive(false)
                                    .desired_width(text_width),
                            );

                            let toggle_label = if is_shown { "Hide" } else { "Show" };
                            if ui
                                .add(
                                    egui::Button::new(toggle_label)
                                        .min_size(egui::vec2(btn_width, 0.)),
                                )
                                .clicked()
                            {
                                if is_shown {
                                    ui.data_mut(|d| d.remove::<Password>(shown_pw_id));
                                } else {
                                    spawn_fetch(
                                        self.rt,
                                        self.db.clone(),
                                        self.lot.clone(),
                                        self.record_uuid.clone(),
                                        pw_inbox.sender(),
                                        PasswordEvent::Show,
                                    );
                                }
                            }
                        });
                    });
            }
        })
        .response
    }
}

fn spawn_fetch(
    rt: &Runtime,
    db: Arc<Database>,
    lot: Arc<Lot>,
    record_uuid: Uuid<Record>,
    tx: egui_inbox::UiInboxSender<PasswordEvent>,
    wrap: fn(Password) -> PasswordEvent,
) {
    rt.spawn(async move {
        // TODO: surface these errors in the UI instead of stderr.
        let record = match Record::show(&db, &lot, &record_uuid).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                eprintln!("record {record_uuid} no longer exists");
                return;
            }
            Err(e) => {
                eprintln!("failed to load record: {e:?}");
                return;
            }
        };
        tx.send(wrap(record.password().clone())).ok();
    });
}
