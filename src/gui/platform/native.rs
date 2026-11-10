use std::path::PathBuf;

use crate::gui::LoadedFile;
use crate::schema::read_order_list;

pub struct Native;

impl super::Platform for Native {
    fn choose_order_list_file(
        &mut self,
        out: &mut Option<LoadedFile>,
        out_err: &mut String,
        open: bool,
    ) -> bool {
        if !open {
            return false;
        }

        let mut dialog = rfd::FileDialog::new().add_filter("json", &["json"]);
        if let Some(current) = &*out
            && let Some(parent) = current.path.parent()
        {
            dialog = dialog.set_directory(parent);
        } else if let Some(dir) = openttd_order_lists_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(file) = dialog.pick_file() {
            use std::fmt::Write;
            match read_order_list(&file) {
                Ok(order_list) => {
                    let generation =
                        out.as_ref().map(|loaded| loaded.generation + 1).unwrap_or_default();
                    *out = Some(LoadedFile {
                        path: file,
                        order_list: Box::new(order_list),
                        generation,
                    });
                    out_err.clear();
                }
                Err(err) => {
                    write!(out_err, "{err:?}").unwrap();
                }
            }
            true
        } else {
            false
        }
    }
}

fn openttd_order_lists_dir() -> Option<PathBuf> {
    let mut path = dirs::data_dir()?;
    path.push("openttd");
    path.push("orderlist");
    Some(path)
}
