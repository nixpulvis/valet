use eframe::egui::{self, ViewportCommand};
use egui_inbox::UiInbox;
use rand_core::{OsRng, RngCore};
use std::{env, sync::Arc};
use tokio::runtime;
use valet::prelude::*;

fn generate_password() -> String {
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

const MIN_SIZE: [f32; 2] = [200., 168.];
const UNLOCKED_DEFAULT_SIZE: [f32; 2] = [400., 600.];

struct PasswordInput<'a> {
    // TODO: Use a Password type
    text: &'a mut String,
    visible: &'a mut bool,
    reserved_right: f32,
}

impl<'a> PasswordInput<'a> {
    fn new(text: &'a mut String, visible: &'a mut bool) -> Self {
        Self {
            text,
            visible,
            reserved_right: 0.0,
        }
    }

    fn reserved_right(mut self, width: f32) -> Self {
        self.reserved_right = width;
        self
    }
}

impl egui::Widget for PasswordInput<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;

            // Measure the wider of "Show"/"Hide" so the width is stable when toggling.
            // Include interact_size.x minimum to match egui's actual button sizing.
            let font_id = egui::TextStyle::Button.resolve(ui.style());
            let btn_width = (ui.fonts(|f| {
                ["Show", "Hide"]
                    .iter()
                    .map(|s| {
                        f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE)
                            .rect
                            .width()
                    })
                    .fold(0.0_f32, f32::max)
            }) + ui.spacing().button_padding.x * 2.0)
                .max(ui.spacing().interact_size.x)
                .ceil();

            // TextEdit is added first so tab order is: input → toggle button.
            let reserved = btn_width
                + spacing * 2.
                + if self.reserved_right > 0.0 {
                    self.reserved_right + spacing
                } else {
                    0.0
                };
            let text_width = (ui.available_width() - reserved).max(0.0);
            let response = ui.add(
                egui::TextEdit::singleline(self.text)
                    .password(!*self.visible)
                    .desired_width(text_width),
            );
            let label = if *self.visible { "Hide" } else { "Show" };
            if ui.button(label).clicked() {
                *self.visible = !*self.visible;
            }
            response
        })
        .inner
    }
}

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
        Box::new(|ctx| Ok(Box::new(ValetApp::new(ctx)))),
    )
    .expect("eframe run failed");
}

struct ValetApp {
    db_url: String,
    rt: runtime::Runtime,

    user: Option<Arc<User>>,

    // TODO: This should be it's own widget.
    username: String,
    password: PasswordBuf,
    show_password: bool,
    login_inbox: UiInbox<User>,

    // TODO: Delete me.
    mock_inbox: UiInbox<Vec<(Lot, Vec<Record>)>>,
    lots: Vec<(Lot, Vec<Record>)>,

    show_new_record: bool,
    new_label: String,
    new_value: String,
    show_new_value: bool,

    search: String,
    lock_label: String,
}

impl ValetApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut dir = env::current_exe().unwrap();
        dir.pop();
        dir.pop();
        let dir = String::from(dir.to_str().unwrap());
        let db_url = format!("sqlite://{}/valet.sqlite?mode=rwc", dir);
        dbg!(&db_url);
        ValetApp {
            db_url,
            rt: runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),

            user: None,

            username: "".into(),
            password: PasswordBuf::empty(),
            show_password: false,
            login_inbox: UiInbox::new(),

            mock_inbox: UiInbox::new(),
            lots: Vec::new(),

            show_new_record: false,
            new_label: String::new(),
            new_value: String::new(),
            show_new_value: false,

            search: String::new(),
            lock_label: String::new(),
        }
    }
}

impl eframe::App for ValetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Fill the entire screen with the panel background before any panels render,
        // preventing a dark flash on startup or window resize.
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            ctx.screen_rect(),
            egui::CornerRadius::ZERO,
            ctx.style().visuals.panel_fill,
        );

        egui::TopBottomPanel::top("my_panel").show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if self.user.is_some() {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let unlocked_width = ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            "Unlocked".into(),
                                            ui.style().text_styles[&egui::TextStyle::Button]
                                                .clone(),
                                            egui::Color32::WHITE,
                                        )
                                        .rect
                                        .width()
                                    });
                                    let lock_btn = ui.add(
                                        egui::Button::new(&self.lock_label)
                                            .frame(false)
                                            .min_size(egui::vec2(unlocked_width, 0.)),
                                    );
                                    if lock_btn.hovered() {
                                        ui.ctx().output_mut(|o| {
                                            o.cursor_icon = egui::CursorIcon::PointingHand
                                        });
                                        self.lock_label = "Lock".into();
                                    } else {
                                        self.lock_label = "Unlocked".into();
                                    }
                                    if lock_btn.clicked() {
                                        self.user = None;
                                        self.lots.clear();
                                        self.show_new_record = false;
                                        self.new_label.clear();
                                        self.new_value.clear();
                                        self.search.clear();
                                        self.lock_label = "Unlocked".into();
                                        self.login_inbox = UiInbox::new();
                                        ctx.send_viewport_cmd(ViewportCommand::InnerSize(
                                            MIN_SIZE.into(),
                                        ));
                                        ctx.send_viewport_cmd(ViewportCommand::Resizable(false));
                                    }
                                    if ui.button("New").clicked() {
                                        self.show_new_record = true;
                                    }
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.search)
                                            .hint_text("Search")
                                            .desired_width(f32::INFINITY),
                                    );
                                },
                            );
                        } else {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label("Locked");
                                },
                            );
                        }
                    });
                });
        });
        if let Some(user) = self.user.clone() {
            if self.lots.is_empty() {
                let db_url = self.db_url.clone();
                let tx = self.mock_inbox.sender();
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

            if let Some(lots) = self.mock_inbox.read(ctx).last() {
                self.lots = lots;
            }

            // Snapshot the records for the main lot to avoid borrow conflicts in the closure.
            let main_lot_records: Option<Vec<(String, String)>> = self
                .lots
                .iter()
                .find(|(l, _)| l.name() == DEFAULT_LOT)
                .map(|(_, records)| {
                    records
                        .iter()
                        .map(|r| (r.data().label().to_owned(), r.password().to_owned()))
                        .collect()
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(ctx.style().visuals.panel_fill))
                .show(ctx, |ui| {
                    if self.show_new_record {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.label("Label:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_label)
                                        .desired_width(f32::INFINITY),
                                );
                                ui.label("Value:");
                                ui.horizontal(|ui| {
                                    let font_id = egui::TextStyle::Button.resolve(ui.style());
                                    let gen_width = (ui.fonts(|f| {
                                        f.layout_no_wrap(
                                            "Generate".to_string(),
                                            font_id,
                                            egui::Color32::WHITE,
                                        )
                                        .rect
                                        .width()
                                    }) + ui.spacing().button_padding.x * 2.0)
                                        .max(ui.spacing().interact_size.x);
                                    ui.add(
                                        PasswordInput::new(
                                            &mut self.new_value,
                                            &mut self.show_new_value,
                                        )
                                        .reserved_right(gen_width),
                                    );
                                    if ui.button("Generate").clicked() {
                                        self.new_value = generate_password();
                                    }
                                });
                                ui.add_space(4.);
                                ui.horizontal(|ui| {
                                    let can_add =
                                        !self.new_label.is_empty() && !self.new_value.is_empty();
                                    if ui
                                        .add_enabled(can_add, egui::Button::new("Add Record"))
                                        .clicked()
                                    {
                                        let db_url = self.db_url.clone();
                                        let tx = self.mock_inbox.sender();
                                        let label = std::mem::take(&mut self.new_label);
                                        let value = std::mem::take(&mut self.new_value);
                                        self.show_new_record = false;
                                        self.show_new_value = false;
                                        self.lots.clear();
                                        self.rt.spawn(async move {
                                            let db = Database::new(&db_url)
                                                .await
                                                .expect("error getting database");
                                            if let Some(lot) = Lot::load(&db, DEFAULT_LOT, &user)
                                                .await
                                                .expect("failed to load main lot")
                                            {
                                                let record = Record::new(
                                                    &lot,
                                                    RecordData::plain(&label, &value),
                                                );
                                                record
                                                    .upsert(&db, &lot)
                                                    .await
                                                    .expect("failed to save record");
                                            }
                                            let lots = user
                                                .lots(&db)
                                                .await
                                                .expect("failed to reload lots");
                                            let mut lots_with_records = Vec::new();
                                            for lot in lots {
                                                let records = lot
                                                    .records(&db)
                                                    .await
                                                    .expect("failed to load records");
                                                lots_with_records.push((lot, records));
                                            }
                                            tx.send(lots_with_records).ok();
                                        });
                                    }
                                    if ui.button("Cancel").clicked() {
                                        self.show_new_record = false;
                                        self.show_new_value = false;
                                        self.new_label.clear();
                                        self.new_value.clear();
                                    }
                                });
                            }); // inner_margin Frame
                        ui.separator();
                    }

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin {
                                left: 8,
                                right: 8,
                                top: 4,
                                bottom: 0,
                            })
                            .show(ui, |ui| {
                                let query = self.search.to_lowercase();
                                match &main_lot_records {
                                    None => {
                                        ui.label("Loading...");
                                    }
                                    Some(records) if records.is_empty() => {
                                        ui.label("No records yet.");
                                    }
                                    Some(records) => {
                                        let mut any = false;
                                        for (label, password) in records {
                                            if query.is_empty()
                                                || label.to_lowercase().contains(&query)
                                            {
                                                ui.horizontal(|ui| {
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            let btn = ui.button("Copy");
                                                            ui.with_layout(
                                                                egui::Layout::left_to_right(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.add(
                                                                        egui::Label::new(label)
                                                                            .truncate(),
                                                                    );
                                                                },
                                                            );
                                                            if btn.clicked() {
                                                                ui.ctx()
                                                                    .copy_text(password.clone());
                                                            }
                                                        },
                                                    );
                                                });
                                                ui.separator();
                                                any = true;
                                            }
                                        }
                                        if !any {
                                            ui.label("No matching records.");
                                        }
                                    }
                                }
                            }); // inner_margin Frame
                    });
                });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(user) = self.login_inbox.read(ui).last() {
                    self.user = Some(Arc::new(user));
                    // TODO: Do we clear the username or not?
                    self.password = PasswordBuf::empty();
                    self.show_password = false;
                    ui.ctx().send_viewport_cmd(ViewportCommand::InnerSize(
                        UNLOCKED_DEFAULT_SIZE.into(),
                    ));
                    ui.ctx().send_viewport_cmd(ViewportCommand::Resizable(true));
                }

                ui.label("Username:");
                let username_re = ui.add(egui::TextEdit::singleline(&mut self.username));
                ui.label("Password:");
                let password_re = ui.add(PasswordInput::new(
                    self.password.as_mut(),
                    &mut self.show_password,
                ));
                ui.add_space(5.);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Unlock")).clicked()
                        || password_re.lost_focus()
                            && username_re.ctx.input(|i| i.key_pressed(egui::Key::Enter))
                        || username_re.lost_focus()
                            && password_re.ctx.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                        let username = self.username.clone();
                        let password = self.password.clone();
                        let db_url = self.db_url.clone();
                        let tx = self.login_inbox.sender();
                        self.rt.spawn(async move {
                            let db = Database::new(&db_url)
                                .await
                                .expect("error getting database");
                            let user = User::load(&db, &username, pw!(password))
                                .await
                                .expect("TODO");
                            if user.validate() {
                                tx.send(user).ok();
                            }
                        });
                    }
                    if ui.add(egui::Button::new("Create")).clicked() {
                        // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                        let username = self.username.clone();
                        let password = self.password.clone();
                        let db_url = self.db_url.clone();
                        let tx = self.login_inbox.sender();
                        self.rt.spawn(async move {
                            let db = Database::new(&db_url).await.expect("error getting DB");
                            let user = User::new(&username, pw!(password))
                                .expect("TODO")
                                .register(&db)
                                .await
                                .expect("TODO");
                            Lot::new(DEFAULT_LOT)
                                .save(&db, &user)
                                .await
                                .expect("failed to save lot");
                            if user.validate() {
                                tx.send(user).ok();
                            }
                        });
                    }
                })
            });
        }
    }
}
