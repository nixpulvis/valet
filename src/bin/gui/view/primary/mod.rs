// struct Main {
//     login_inbox: UiInbox<User>,
// }

// impl Widget for Main {
//     fn ui(self, ui: &mut Ui) -> Response {
//         unimplemented!()
//     }
// }

// enum MainInner {
//     Locked(Locked),
//     Unlocked(Unlocked),
// }

// TODO: Does this need to be explicit
pub const LOCKED_SIZE: [f32; 2] = [200., 120.];
pub const UNLOCKED_DEFAULT_SIZE: [f32; 2] = [400., 600.];

mod locked;
pub use self::locked::Locked;
mod unlocked;
pub use self::unlocked::Unlocked;
