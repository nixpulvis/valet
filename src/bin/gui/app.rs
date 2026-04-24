use crate::view::{Locked, Unlocked, View};
use eframe::egui;
use egui_inbox::UiInbox;
use std::sync::Arc;
use tokio::runtime;
use valet::db::Database;
use valet::protocol::EmbeddedHandler;

pub struct App {
    pub(crate) client: Arc<EmbeddedHandler>,
    pub(crate) rt: runtime::Runtime,
    pub(crate) active_user: Option<String>,
    pub(crate) login_inbox: UiInbox<String>,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let rt = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = rt
            .block_on(Database::new(&valet::db::default_url()))
            .expect("failed to open database");
        let client = Arc::new(EmbeddedHandler::new(db, rt.handle()));
        App {
            client,
            rt,
            active_user: None,
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

        if self.active_user.is_some() {
            Unlocked::new(
                &self.client,
                &self.rt,
                &mut self.active_user,
                &mut self.login_inbox,
            )
            .show(ctx);
        } else {
            Locked::new(
                &self.client,
                &self.rt,
                &mut self.active_user,
                &self.login_inbox,
            )
            .show(ctx);
        }
    }
}
