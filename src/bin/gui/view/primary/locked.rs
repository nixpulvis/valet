use crate::{
    view::{View, primary::UNLOCKED_DEFAULT_SIZE},
    widget::PasswordInput,
};
use eframe::egui::{Button, CentralPanel, Context, Id, Key, TextEdit, ViewportCommand};
use egui_inbox::UiInbox;
use std::sync::Arc;
use tokio::runtime::Runtime;
use valet::SendHandler;
use valet::password::Password;
use valet::protocol::EmbeddedHandler;
use valet::protocol::message::{Register, Unlock};

pub struct Locked<'a> {
    // TODO: come up with a better organization for these shared values.
    client: &'a Arc<EmbeddedHandler>,
    rt: &'a Runtime,
    active_user: &'a mut Option<String>,
    login_inbox: &'a UiInbox<String>,
}

impl<'a> Locked<'a> {
    pub fn new(
        client: &'a Arc<EmbeddedHandler>,
        rt: &'a Runtime,
        active_user: &'a mut Option<String>,
        login_inbox: &'a UiInbox<String>,
    ) -> Self {
        Locked {
            client,
            rt,
            active_user,
            login_inbox,
        }
    }
}

impl<'a> View for Locked<'a> {
    fn show(&mut self, ctx: &Context) {
        let id = Id::new("locked_view");
        let mut state = State::load(ctx, id);

        CentralPanel::default().show(ctx, |ui| {
            if let Some(username) = self.login_inbox.read(ui).last() {
                *self.active_user = Some(username);

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
            // TODO: Update PasswordInput to operate on Password directly.
            let password_re = ui.add(PasswordInput::new(&mut state.password));
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
                    let client = self.client.clone();
                    let tx = self.login_inbox.sender();
                    self.rt.spawn(async move {
                        if client
                            .call(Unlock {
                                username: username.clone(),
                                password,
                            })
                            .await
                            .is_ok()
                        {
                            tx.send(username).ok();
                        }
                    });
                }
                if ui.add(Button::new("Create")).clicked() {
                    // XXX: This is obviously hacky, but I don't want to deal with sharing things now.
                    let username = state.username.clone();
                    let password = state.password.clone();
                    let client = self.client.clone();
                    let tx = self.login_inbox.sender();
                    self.rt.spawn(async move {
                        if client
                            .call(Register {
                                username: username.clone(),
                                password,
                            })
                            .await
                            .is_ok()
                        {
                            tx.send(username).ok();
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
    password: Password,
}

impl State {
    fn load(ctx: &Context, id: Id) -> Self {
        State {
            username: ctx.data(|d| d.get_temp(id.with("username")).unwrap_or_default()),
            password: ctx.data(|d| {
                d.get_temp(id.with("password"))
                    .unwrap_or(Password::default())
            }),
        }
    }

    fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| {
            d.insert_temp(id.with("username"), self.username);
            d.insert_temp(id.with("password"), self.password);
        });
    }
}
