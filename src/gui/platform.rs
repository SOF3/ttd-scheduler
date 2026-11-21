use crate::gui::{FileList, PersistData};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Native;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Web;

pub trait Platform {
    fn choose_order_list_file(&mut self, ctx: &egui::Context, files: &mut FileList, open: bool);
    fn read_persist_data(&self) -> anyhow::Result<PersistData>;
    fn write_persist_data(&self, data: &PersistData) -> anyhow::Result<()>;
}
