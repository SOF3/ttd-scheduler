use crate::gui::{FileList, PersistData};

mod native;
pub use native::Native;

pub trait Platform {
    fn choose_order_list_file(&mut self, ctx: &egui::Context, files: &mut FileList, open: bool);
    fn read_persist_data(&self) -> anyhow::Result<PersistData>;
    fn write_persist_data(&self, data: &PersistData) -> anyhow::Result<()>;
}
