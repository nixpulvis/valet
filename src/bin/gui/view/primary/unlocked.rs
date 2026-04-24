use crate::{
    util::button_width,
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
use valet::Handler;
use valet::{
    Record,
    lot::DEFAULT_LOT,
    password::Password,
    protocol::{Client, embedded::Embedded},
    record::{Label, Query},
    uuid::Uuid,
};

type Index = Vec<(Uuid<Record>, Label)>;

pub struct Unlocked<'a> {
    // TODO: come up with a better organization for these shared values.
    client: &'a Arc<Client<Embedded>>,
    rt: &'a Runtime,
    active_user: &'a mut Option<String>,
    login_inbox: &'a mut UiInbox<String>,
}

impl<'a> Unlocked<'a> {
    pub fn new(
        client: &'a Arc<Client<Embedded>>,
        rt: &'a Runtime,
        active_user: &'a mut Option<String>,
        login_inbox: &'a mut UiInbox<String>,
    ) -> Self {
        Unlocked {
            client,
            rt,
            active_user,
            login_inbox,
        }
    }
}

impl<'a> View for Unlocked<'a> {
    fn show(&mut self, ctx: &Context) {
        let id = Id::new("unlocked_view");
        let mut state = State::load(ctx, id);

        let username = if let Some(u) = self.active_user.as_ref() {
            u.clone()
        } else {
            unreachable!();
        };

        // The persistent `index` is empty until the first refresh
        // lands. A sentinel on `loaded` distinguishes "not yet loaded"
        // from "loaded, empty".
        if !*state.loaded.read().unwrap() {
            let client = self.client.clone();
            let tx = state.index_inbox.sender();
            let username_c = username.clone();
            self.rt.spawn(async move {
                let entries = client_list_all(&client, &username_c).await;
                tx.send(entries).ok();
            });
            *state.loaded.write().unwrap() = true;
        }

        if let Some(entries) = state.index_inbox.read(ctx).last() {
            *state.index.write().unwrap() = entries;
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
                                let client = self.client.clone();
                                let uname = username.clone();
                                self.rt.spawn(async move {
                                    let _ = client.lock(uname).await;
                                });
                                *self.active_user = None;
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
                                state.new_password = Password::generate();
                            }
                        });
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            let can_add =
                                !state.new_label.is_empty() && !state.new_password.is_empty();
                            if ui.add_enabled(can_add, Button::new("Add Record")).clicked() {
                                let client = self.client.clone();
                                let new_label = state.new_label.clone();
                                let new_password = state.new_password.clone();
                                let username_c = username.clone();
                                let refresh_tx = state.index_inbox.sender();
                                state.show_new_record = false;
                                state.new_label = String::new();
                                state.new_password = Password::default();
                                self.rt.spawn(async move {
                                    match Query::from_str(&new_label).and_then(Query::into_path) {
                                        Ok(path) => {
                                            // create_record upserts by label
                                            // name, so resaving the same label
                                            // extends history in place.
                                            if let Err(e) = client
                                                .create_record(
                                                    username_c.clone(),
                                                    DEFAULT_LOT.to_owned(),
                                                    path.label,
                                                    new_password,
                                                    Default::default(),
                                                )
                                                .await
                                            {
                                                // TODO: surface in UI.
                                                eprintln!("failed to save record: {e}");
                                                return;
                                            }
                                            let entries =
                                                client_list_all(&client, &username_c).await;
                                            refresh_tx.send(entries).ok();
                                        }
                                        Err(error) => {
                                            // TODO: We need error flashes in the UI.
                                            eprintln!("{error}: {}", new_label)
                                        }
                                    }
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
                            // Bare input is a case-insensitive literal prefix
                            // match on the label name. A leading `~` opts into
                            // the full Query grammar (regex name, `<k=v>`
                            // extras filters, etc.), same as CLI `get`.
                            let search = state.search.trim();
                            let query = if search.is_empty() {
                                Ok(None)
                            } else if search.starts_with('~') {
                                Query::from_str(search).map(Some)
                            } else {
                                Ok(Some(Query::label_prefix(search, true)))
                            };
                            match query {
                                Err(e) => {
                                    ui.label(format!("Invalid query: {e}"));
                                }
                                Ok(query) => {
                                    let entries = state.index.read().unwrap().clone();
                                    if entries.is_empty() {
                                        ui.label("No records yet.");
                                    } else {
                                        let mut any = false;
                                        for (record_uuid, label) in &entries {
                                            if let Some(q) = &query
                                                && !q.matches_label(label)
                                            {
                                                continue;
                                            }
                                            ui.add(RecordRow::new(
                                                label,
                                                record_uuid,
                                                username.clone(),
                                                self.client,
                                                self.rt,
                                            ));
                                            ui.separator();
                                            any = true;
                                        }
                                        if !any {
                                            ui.label("No matching records.");
                                        }
                                    }
                                }
                            }
                        });
                });
            });
        state.store(ctx, id);
    }
}

async fn client_list_all(client: &Arc<Client<Embedded>>, username: &str) -> Index {
    // An empty query list on the handler means "every record in every
    // lot the user has access to". The UI then filters the main lot in
    // the render path.
    client
        .list(username.to_owned(), Vec::new())
        .await
        .unwrap_or_default()
}

struct State {
    index_inbox: Arc<UiInbox<Index>>,
    index: Arc<RwLock<Index>>,
    loaded: Arc<RwLock<bool>>,
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
            index_inbox: ctx.data(|d| {
                d.get_temp::<Arc<UiInbox<Index>>>(id.with("index_inbox"))
                    .unwrap_or_default()
            }),
            index: ctx.data(|d| d.get_temp(id.with("index")).unwrap_or_default()),
            loaded: ctx.data(|d| d.get_temp(id.with("loaded")).unwrap_or_default()),
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
            d.insert_temp(id.with("index_inbox"), self.index_inbox);
            d.insert_temp(id.with("index"), self.index);
            d.insert_temp(id.with("loaded"), self.loaded);
            d.insert_temp(id.with("search"), self.search);
            d.insert_temp(id.with("lock_label"), self.lock_label);
            d.insert_temp(id.with("show_new_record"), self.show_new_record);
            d.insert_temp(id.with("new_label"), self.new_label);
            d.insert_temp(id.with("new_password"), self.new_password);
        });
    }
}
