use crate::{
    view::{View, primary::UNLOCKED_DEFAULT_SIZE},
    widget::PasswordInput,
};
use eframe::egui::{Button, CentralPanel, Context, Id, Key, TextEdit, ViewportCommand};
use egui_inbox::UiInbox;
use std::sync::Arc;
use tokio::runtime::Runtime;
use valet::{Lot, User, db::Database, encrypt::PasswordBuf, lot::DEFAULT_LOT, pw};

pub struct Locked<'a> {
    // TODO: come up with a better organization for these shared values.
    db_url: &'a String,
    rt: &'a Runtime,
    user: &'a mut Option<Arc<User>>,
    login_inbox: &'a UiInbox<User>,
}

impl<'a> Locked<'a> {
    pub fn new(
        db_url: &'a String,
        rt: &'a Runtime,
        user: &'a mut Option<Arc<User>>,
        login_inbox: &'a UiInbox<User>,
    ) -> Self {
        Locked {
            db_url,
            rt,
            user,
            login_inbox,
        }
    }
}

impl<'a> View for Locked<'a> {
    fn show(&mut self, ctx: &Context) {
        let id = Id::new("locked_view");
        let mut state = State::load(ctx, id);

        CentralPanel::default().show(ctx, |ui| {
            if let Some(user) = self.login_inbox.read(ui).last() {
                *self.user = Some(Arc::new(user));

                // TODO: Make some kind of transition function to handle the
                // logic between Locked and Unlocked. Unlocked should have one
                // as well. In general with more than two view options this
                // requires more thought.

                // TODO: Do we clear the username or not?
                ui.ctx().data_mut(|d| d.clear());
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::InnerSize(UNLOCKED_DEFAULT_SIZE.into()));
                ui.ctx().send_viewport_cmd(ViewportCommand::Resizable(true));
            }

            ui.label("Username:");
            let username_re = ui.add(TextEdit::singleline(&mut state.username));
            ui.label("Password:");
            let password_re = ui.add(PasswordInput::new(
                state.password.as_mut(),
                &mut state.show_password,
            ));
            ui.add_space(5.);
            ui.horizontal(|ui| {
                if ui.add(Button::new("Unlock")).clicked()
                    || password_re.lost_focus()
                        && username_re.ctx.input(|i| i.key_pressed(Key::Enter))
                    || username_re.lost_focus()
                        && password_re.ctx.input(|i| i.key_pressed(Key::Enter))
                {
                    // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                    let username = state.username.clone();
                    let password = state.password.clone();
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
                if ui.add(Button::new("Create")).clicked() {
                    // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                    let username = state.username.clone();
                    let password = state.password.clone();
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
        state.store(ctx, id);
    }
}

struct State {
    username: String,
    password: PasswordBuf,
    show_password: bool,
}

impl State {
    fn load(ctx: &Context, id: Id) -> Self {
        State {
            username: ctx.data(|d| d.get_temp(id.with("username")).unwrap_or_default()),
            password: ctx.data(|d| {
                d.get_temp(id.with("password"))
                    .unwrap_or(PasswordBuf::empty())
            }),
            show_password: ctx.data(|d| d.get_temp(id.with("show_password")).unwrap_or_default()),
        }
    }

    fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| {
            d.insert_temp(id.with("username"), self.username);
            d.insert_temp(id.with("password"), self.password);
            d.insert_temp(id.with("show_password"), self.show_password);
        });
    }
}
