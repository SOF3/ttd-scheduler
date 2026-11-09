use std::path::PathBuf;

use crate::schema::OrderList;

mod native;
pub use native::Native;

pub trait Platform {
    fn choose_order_list_file(
        &mut self,
        out: &mut Option<(PathBuf, Box<OrderList>)>,
        out_err: &mut String,
        open: bool,
    );
}
