use std::path::PathBuf;

use crate::schema::{OrderList, read_order_list};

pub struct Native;

impl super::Platform for Native {
    fn choose_order_list_file(
        &mut self,
        out: &mut Option<(PathBuf, Box<OrderList>)>,
        out_err: &mut String,
        open: bool,
    ) {
        if !open {
            return;
        }

        let mut dialog = rfd::FileDialog::new().add_filter("json", &["json"]);
        if let Some((current, _)) = &*out
            && let Some(parent) = current.parent()
        {
            dialog = dialog.set_directory(parent);
        } else if let Some(dir) = openttd_order_lists_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(file) = dialog.pick_file() {
            use std::fmt::Write;
            match read_order_list(&file) {
                Ok(order_list) => {
                    *out = Some((file, Box::new(order_list)));
                    out_err.clear();
                }
                Err(err) => {
                    write!(out_err, "{err:?}").unwrap();
                }
            }
        }
    }
}

fn openttd_order_lists_dir() -> Option<PathBuf> {
    let mut path = dirs::data_dir()?;
    path.push("openttd");
    path.push("orderlist");
    Some(path)
}
