use crate::gui::LoadedFile;

mod native;
pub use native::Native;

pub trait Platform {
    fn choose_order_list_file(
        &mut self,
        out: &mut Option<LoadedFile>,
        out_err: &mut String,
        open: bool,
    ) -> bool;
}
