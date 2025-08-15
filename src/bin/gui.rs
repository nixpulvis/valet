use eframe::egui::{self, ViewportCommand};
use egui_inbox::UiInbox;
use std::{env, sync::Arc};
use tokio::runtime;
use valet::prelude::*;

const MIN_SIZE: [f32; 2] = [200., 160.];
const MAX_SIZE: [f32; 2] = [400., 350.];

fn main() {
    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_inner_size(MIN_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_max_inner_size(MAX_SIZE);
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
    mock_inbox: UiInbox<Vec<Lot>>,
    lots: Vec<Lot>,
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
        }
    }
}

impl eframe::App for ValetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("my_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.set_width(35.);
                    if self.user.is_some() {
                        if ui.button("Lock").clicked() {
                            self.user = None;
                            self.lots.clear();
                            self.login_inbox = UiInbox::new();
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(MIN_SIZE.into()));
                        }
                    }
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.user.is_some() {
                        ui.label("Unlocked");
                    } else {
                        ui.label("Locked");
                    }
                });
            });
        });
        if let Some(user) = self.user.clone() {
            if self.lots.is_empty() {
                let db_url = self.db_url.clone();
                let tx = self.mock_inbox.sender();
                self.rt.spawn(async move {
                    let db = Database::new(&db_url)
                        .await
                        .expect("error getting database");
                    let lots = user.lots(&db).await.expect("failed to load lots");
                    tx.send(lots).ok();
                });
            }

            if let Some(lots) = self.mock_inbox.read(ctx).last() {
                self.lots = lots;
            }

            egui::CentralPanel::default().show(ctx, |ui| {
                for lot in self.lots.iter() {
                    ui.separator();
                    ui.label(format!("Lot: {}", lot.name()));
                    ui.separator();
                    for record in lot.records().iter() {
                        ui.label(format!("{}", record.data()));
                    }
                }
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(user) = self.login_inbox.read(ui).last() {
                    self.user = Some(Arc::new(user));
                    // TODO: Do we clear the username or not?
                    self.password = PasswordBuf::empty();
                    self.show_password = false;
                }

                ui.label("Username:");
                let username_re = ui.add(egui::TextEdit::singleline(&mut self.username));
                ui.label("Password:");
                let password_re = ui.add(
                    egui::TextEdit::singleline(self.password.as_mut())
                        .password(!self.show_password),
                );
                ui.checkbox(&mut self.show_password, "Show password");
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
