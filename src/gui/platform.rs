use crate::gui::FileList;

mod native;
pub use native::Native;

pub trait Platform {
    fn choose_order_list_file(&mut self, ctx: &egui::Context, files: &mut FileList, open: bool);
}
