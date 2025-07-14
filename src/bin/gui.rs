use eframe::egui::{self, RichText, ViewportCommand};
use egui_inbox::UiInbox;
use tokio::runtime;
use valet::db::{DEFAULT_URL, Database, Lots, Users};
use valet::user::User;

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
    logged_in: bool,
    // TODO: This should be it's own widget.
    username: String,
    password: String,
    show_password: bool,
    login_inbox: UiInbox<(User, String)>,
    rt: runtime::Runtime,
    user: Option<User>,
    lot: Option<String>,
}

impl ValetApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        ValetApp {
            logged_in: false,
            // XXX: prefilled for faster testing
            username: "nixpulvis".into(),
            password: "password".into(),
            show_password: false,
            rt: runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
            login_inbox: UiInbox::new(),
            user: None,
            lot: None,
        }
    }
}

impl eframe::App for ValetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("my_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.set_width(35.);
                    if self.logged_in {
                        if ui.button("Lock").clicked() {
                            self.logged_in = false;
                            self.lot = None;
                            self.user = None;
                            self.login_inbox = UiInbox::new();
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(MIN_SIZE.into()));
                        }
                    }
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.logged_in {
                        ui.label("Unlocked");
                    } else {
                        ui.label("Locked");
                    }
                });
            });
        });
        if self.logged_in {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(user) = &self.user {
                    ui.label(RichText::new(&user.username).strong());
                    if let Some(ref mut msg) = self.lot {
                        let mut size = ui.available_size();
                        size[1] -= 20.;
                        ui.add_sized(size, egui::TextEdit::multiline(msg));
                        if ui.add(egui::Button::new("Save")).clicked() {
                            let username = self.username.clone();
                            let encrypted = user
                                .credential()
                                .encrypt(msg.as_bytes())
                                .expect("error encrypting");
                            self.rt.spawn(async move {
                                let db =
                                    Database::new(DEFAULT_URL).await.expect("error getting DB");
                                Lots::create(&db, &username, &encrypted)
                                    .await
                                    .expect("TODO");
                            });
                        }
                    } else {
                        ui.label("Error loading encrypted data.");
                        if ui.add(egui::Button::new("Load Main Lot")).clicked() {}
                    }
                }
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some((user, msg)) = self.login_inbox.read(ui).last() {
                    self.password = "".into();
                    self.show_password = false;
                    self.logged_in = true;
                    self.user = Some(user);
                    self.lot = Some(msg.into());
                }

                ui.label("Username:");
                let username_re = ui.add(egui::TextEdit::singleline(&mut self.username));
                ui.label("Password:");
                let password_re = ui.add(
                    egui::TextEdit::singleline(&mut self.password).password(!self.show_password),
                );
                ui.checkbox(&mut self.show_password, "Show password");
                ui.add_space(5.);
                if ui.add(egui::Button::new("Unlock")).clicked()
                    || password_re.lost_focus()
                        && username_re.ctx.input(|i| i.key_pressed(egui::Key::Enter))
                    || username_re.lost_focus()
                        && password_re.ctx.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                    let username = self.username.clone();
                    let password = self.password.clone();
                    let tx = self.login_inbox.sender();
                    self.rt.spawn(async move {
                        let db = Database::new(DEFAULT_URL).await.expect("error getting DB");
                        let user = Users::get(&db, &username, &password).await.expect("TODO");
                        if user.validate() {
                            let encrypted = Lots::get(&db, &username).await.expect("TODO");
                            let data = user
                                .credential()
                                .decrypt(&encrypted)
                                .expect("error decrypting load");
                            let msg = std::str::from_utf8(&data).expect("error parsing string");
                            tx.send((user, msg.into())).ok();
                        }
                    });
                }
            });
        }
    }
}
