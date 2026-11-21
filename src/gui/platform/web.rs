use std::path::PathBuf;
use std::sync::atomic;

use anyhow::Context;
use futures::channel::oneshot;
use futures::{StreamExt, TryStreamExt, stream};

use crate::gui::{FileId, FileList, LoadedFile, PersistData};
use crate::schema::read_order_lists_from_str;

#[derive(Default)]
pub struct Web {
    load_task: Option<oneshot::Receiver<Vec<anyhow::Result<LoadedFile>>>>,
}

impl super::Platform for Web {
    fn choose_order_list_file(&mut self, ctx: &egui::Context, files: &mut FileList, open: bool) {
        if open {
            let ctx = ctx.clone();
            let next_file_id = files.next_file_id.clone();

            let dialog = rfd::AsyncFileDialog::new().add_filter("orderlist", &["json"]);
            let (tx, rx) = oneshot::channel();
            self.load_task = Some(rx);

            wasm_bindgen_futures::spawn_local(async move {
                let files = dialog.pick_files().await;
                let results: Vec<anyhow::Result<_>> = stream::iter(files.into_iter().flatten())
                    .then(async |file| {
                        let contents = file.read().await;
                        let contents =
                            String::from_utf8(contents).context("file contains invalid utf-8")?;
                        let lists = read_order_lists_from_str(&contents)?;
                        anyhow::Ok(stream::iter(lists.into_iter().map(
                            move |(subpaths, order_list)| {
                                let mut path = PathBuf::from(file.file_name());
                                for subpath in subpaths {
                                    path.push(subpath);
                                }
                                anyhow::Ok((path, order_list))
                            },
                        )))
                    })
                    .try_flatten()
                    .map(|result| {
                        let (path, order_list) = result?;
                        Ok(LoadedFile {
                            path,
                            order_list,
                            file_id: FileId(next_file_id.fetch_add(1, atomic::Ordering::Relaxed)),
                        })
                    })
                    .collect()
                    .await;
                _ = tx.send(results);
                ctx.request_repaint();
            });
        }

        let results = if let Some(task) = &mut self.load_task {
            match task.try_recv() {
                Ok(Some(results)) => {
                    self.load_task = None;
                    Some(results)
                }
                Ok(None) => None,
                Err(_) => {
                    self.load_task = None;
                    None
                }
            }
        } else {
            None
        };
        if let Some(results) = results {
            for result in results {
                match result {
                    Ok(file) => files.files.push(file),
                    Err(err) => files.last_err = format!("{err:?}"),
                }
            }
            files.generation += 1;
        }
    }

    fn read_persist_data(&self) -> anyhow::Result<PersistData> {
        let storage = web_sys::window()
            .context("dom window does not exist")?
            .local_storage()
            .map_err(|_| anyhow::anyhow!("get local storage"))?
            .context("local storage not available")?;
        let contents = storage
            .get_item(PERSIST_KEY)
            .map_err(|_| anyhow::anyhow!("get local storage value"))?
            .context("no previous persisted settings")?;
        serde_json::from_str(&contents).context("parse persisted contents")
    }

    fn write_persist_data(&self, data: &PersistData) -> anyhow::Result<()> {
        let storage = web_sys::window()
            .context("dom window does not exist")?
            .local_storage()
            .map_err(|_| anyhow::anyhow!("get local storage"))?
            .context("local storage not available")?;
        let contents = serde_json::to_string(data).context("serialize persist data")?;
        storage
            .set_item(PERSIST_KEY, &contents)
            .map_err(|_| anyhow::anyhow!("set local storage value"))
    }
}

const PERSIST_KEY: &str = "ttd-scheduler-persist";
