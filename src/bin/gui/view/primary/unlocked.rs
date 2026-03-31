use crate::{
    util::{button_width, generate_password},
    view::{View, primary::LOCKED_SIZE},
    widget::{PasswordInput, RecordRow},
};
use eframe::egui::{
    self, Align, Button, CentralPanel, Context, CursorIcon, Frame, Id, Layout, Margin, ScrollArea,
    TextEdit, ViewportCommand,
};
use egui_inbox::UiInbox;
use std::{
    str::FromStr,
    sync::{Arc, RwLock},
};
use tokio::runtime::Runtime;
use valet::{
    Lot, Record, User,
    db::Database,
    lot::DEFAULT_LOT,
    password::Password,
    record::{Data, Label},
};

type Store = Vec<(Lot, Vec<Record>)>;

pub struct Unlocked<'a> {
    // TODO: come up with a better organization for these shared values.
    db_url: &'a String,
    rt: &'a Runtime,
    user: &'a mut Option<Arc<User>>,
    login_inbox: &'a mut UiInbox<User>,
}

impl<'a> Unlocked<'a> {
    pub fn new(
        db_url: &'a String,
        rt: &'a Runtime,
        user: &'a mut Option<Arc<User>>,
        login_inbox: &'a mut UiInbox<User>,
    ) -> Self {
        Unlocked {
            db_url,
            rt,
            user,
            login_inbox,
        }
    }
}

impl<'a> View for Unlocked<'a> {
    fn show(&mut self, ctx: &Context) {
        let id = Id::new("unlocked_view");
        let mut state = State::load(ctx, id);

        // TODO: Come up with a better way.
        let user = if let Some(u) = self.user {
            u.clone()
        } else {
            unreachable!();
        };

        if state.store.read().unwrap().is_empty() {
            let db_url = self.db_url.clone();
            let tx = state.store_inbox.sender();
            let user2 = user.clone();
            self.rt.spawn(async move {
                let db = Database::new(&db_url)
                    .await
                    .expect("error getting database");
                let lots = user2.lots(&db).await.expect("failed to load lots");
                let mut lots_with_records = Vec::new();
                for lot in lots {
                    let records = lot.records(&db).await.expect("failed to load records");
                    lots_with_records.push((lot, records));
                }
                tx.send(lots_with_records).ok();
            });
        }

        if let Some(store) = state.store_inbox.read(ctx).last() {
            *state.store.write().unwrap() = store;
        }

        egui::TopBottomPanel::top("my_panel").show(ctx, |ui| {
            Frame::NONE
                .inner_margin(Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let lock_btn =
                                ui.add(Button::new(&state.lock_label).frame(false).min_size(
                                    egui::vec2(button_width(ui, &["Lock", "Unlocked"]), 0.),
                                ));
                            if lock_btn.hovered() {
                                ui.ctx()
                                    .output_mut(|o| o.cursor_icon = CursorIcon::PointingHand);
                                state.lock_label = "Lock".into();
                            } else {
                                state.lock_label = "Unlocked".into();
                            }
                            if lock_btn.clicked() {
                                // TODO: Again this should be in some
                                // sort of transition function between
                                // Unlocked and Locked.

                                *self.user = None;
                                *self.login_inbox = UiInbox::new();
                                // TODO: Do we clear the username or not?
                                ui.ctx().data_mut(|d| d.clear());
                                ui.ctx().send_viewport_cmd(ViewportCommand::InnerSize(
                                    LOCKED_SIZE.into(),
                                ));
                                ui.ctx()
                                    .send_viewport_cmd(ViewportCommand::Resizable(false));
                            }
                            if ui.button("New").clicked() {
                                state.show_new_record = true;
                            }
                            ui.add(
                                TextEdit::singleline(&mut state.search)
                                    .hint_text("Search")
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    });
                });
        });

        CentralPanel::default()
            .frame(Frame::NONE.fill(ctx.style().visuals.panel_fill))
            .show(ctx, |ui| {
                if state.show_new_record {
                    Frame::NONE.inner_margin(Margin::same(8)).show(ui, |ui| {
                        ui.label("Label:");
                        ui.add(
                            TextEdit::singleline(&mut state.new_label).desired_width(f32::INFINITY),
                        );
                        ui.label("Value:");
                        ui.horizontal(|ui| {
                            let gen_width = button_width(ui, &["Generate"]);
                            ui.add(
                                PasswordInput::new(&mut state.new_password)
                                    .reserved_right(gen_width),
                            );
                            if ui.button("Generate").clicked() {
                                state.new_password = generate_password();
                            }
                        });
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            let can_add =
                                !state.new_label.is_empty() && !state.new_password.is_empty();
                            if ui.add_enabled(can_add, Button::new("Add Record")).clicked() {
                                let db_url = self.db_url.clone();
                                let tx = state.store_inbox.sender();
                                let user = user.clone();
                                let new_label = state.new_label.clone();
                                let new_password = state.new_password.clone();
                                state.show_new_record = false;
                                state.new_label = String::new();
                                state.new_password = Password::default();
                                state.store.write().unwrap().clear();
                                self.rt.spawn(async move {
                                    let db = Database::new(&db_url)
                                        .await
                                        .expect("error getting database");
                                    if let Some(lot) = Lot::load(&db, DEFAULT_LOT, &user)
                                        .await
                                        .expect("failed to load main lot")
                                    {
                                        match Label::from_str(&new_label) {
                                            Ok(label) => {
                                                // TODO: Add deleted record to new record's history.
                                                Record::new(&lot, Data::new(label, new_password))
                                                    .upsert(&db, &lot)
                                                    .await
                                                    .expect("failed to save record");
                                            }
                                            Err(error) => {
                                                // TODO: We need error flashes in the UI.
                                                eprintln!("{error:?}: {}", new_label)
                                            }
                                        }
                                    }
                                    // TODO: We probably just need to query this one record.
                                    let lots = user.lots(&db).await.expect("failed to reload lots");
                                    let mut lots_with_records = Vec::new();
                                    for lot in lots {
                                        let records =
                                            lot.records(&db).await.expect("failed to load records");
                                        lots_with_records.push((lot, records));
                                    }
                                    tx.send(lots_with_records).ok();
                                });
                            }
                            if ui.button("Cancel").clicked() {
                                state.show_new_record = false;
                                state.new_label.clear();
                                state.new_password = Password::default();
                            }
                        });
                    }); // inner_margin Frame
                    ui.separator();
                }

                ScrollArea::vertical().show(ui, |ui| {
                    Frame::NONE
                        .inner_margin(Margin {
                            left: 8,
                            right: 8,
                            top: 4,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            let store = state.store.read().unwrap();
                            let main_lot_records: Option<&(Lot, Vec<Record>)> =
                                store.iter().find(|(l, _)| l.name() == DEFAULT_LOT);

                            let query = state.search.to_lowercase();
                            match main_lot_records {
                                None => {
                                    ui.label("Loading...");
                                }
                                Some((_lot, records)) if records.is_empty() => {
                                    ui.label("No records yet.");
                                }
                                Some((_lot, records)) => {
                                    let mut any = false;
                                    for record in records {
                                        if query.is_empty()
                                            || record
                                                .label()
                                                .to_string()
                                                .to_lowercase()
                                                .contains(&query)
                                        {
                                            ui.add(RecordRow::new(record));
                                            ui.separator();
                                            any = true;
                                        }
                                    }
                                    if !any {
                                        ui.label("No matching records.");
                                    }
                                }
                            }
                        });
                });
            });
        state.store(ctx, id);
    }
}

struct State {
    store_inbox: Arc<UiInbox<Store>>,
    store: Arc<RwLock<Store>>,
    search: String,
    lock_label: String,
    // TODO: Move into NewRecord widget
    show_new_record: bool,
    new_label: String,
    new_password: Password,
}

impl State {
    fn load(ctx: &Context, id: Id) -> Self {
        State {
            store_inbox: ctx.data(|d| {
                d.get_temp::<Arc<UiInbox<Store>>>(id.with("store_inbox"))
                    .unwrap_or_default()
            }),
            store: ctx.data(|d| d.get_temp(id.with("store")).unwrap_or_default()),
            search: ctx.data(|d| d.get_temp(id.with("search")).unwrap_or_default()),
            lock_label: ctx.data(|d| d.get_temp(id.with("lock_label")).unwrap_or_default()),
            show_new_record: ctx
                .data(|d| d.get_temp(id.with("show_new_record")).unwrap_or_default()),
            new_label: ctx.data(|d| d.get_temp(id.with("new_label")).unwrap_or_default()),
            new_password: ctx.data(|d| {
                d.get_temp(id.with("new_password"))
                    .unwrap_or(Password::default())
            }),
        }
    }

    fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| {
            d.insert_temp(id.with("store_inbox"), self.store_inbox);
            d.insert_temp(id.with("store"), self.store);
            d.insert_temp(id.with("search"), self.search);
            d.insert_temp(id.with("lock_label"), self.lock_label);
            d.insert_temp(id.with("show_new_record"), self.show_new_record);
            d.insert_temp(id.with("new_label"), self.new_label);
            d.insert_temp(id.with("new_password"), self.new_password);
        });
    }
}
