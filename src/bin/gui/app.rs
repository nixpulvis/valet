use crate::view::{Locked, Unlocked, View};
use eframe::egui;
use egui_inbox::UiInbox;
use std::sync::Arc;
use tokio::runtime;
use valet::prelude::*;

pub struct App {
    db_url: String,
    rt: runtime::Runtime,
    user: Option<Arc<User>>,
    login_inbox: UiInbox<User>,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let db_url = valet::db::default_url();
        App {
            db_url,
            rt: runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
            user: None,
            login_inbox: UiInbox::new(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Fill the entire screen with the panel background before any panels render,
        // preventing a dark flash on startup or window resize.
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            ctx.screen_rect(),
            egui::CornerRadius::ZERO,
            ctx.style().visuals.panel_fill,
        );

        if self.user.is_some() {
            Unlocked::new(
                &self.db_url,
                &self.rt,
                &mut self.user,
                &mut self.login_inbox,
            )
            .show(ctx);
        } else {
            Locked::new(&self.db_url, &self.rt, &mut self.user, &self.login_inbox).show(ctx);
        }
    }
}
