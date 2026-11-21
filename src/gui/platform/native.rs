use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic;

use anyhow::Context;
use futures::executor::block_on;
use futures::{StreamExt, TryStreamExt, stream};

use crate::gui::{FileId, FileList, LoadedFile, PersistData};
use crate::schema::read_order_lists_from_str;

#[derive(Default)]
pub struct Native {
    executor:  smol::LocalExecutor<'static>,
    load_task: Option<smol::Task<Vec<anyhow::Result<LoadedFile>>>>,
}

impl super::Platform for Native {
    fn choose_order_list_file(&mut self, ctx: &egui::Context, files: &mut FileList, open: bool) {
        if open {
            let ctx = ctx.clone();
            let next_file_id = files.next_file_id.clone();

            let mut dialog = rfd::AsyncFileDialog::new().add_filter("orderlist", &["json"]);
            if let Some(dir) = files
                .files
                .last()
                .and_then(|last| last.path.parent().map(PathBuf::from))
                .or_else(openttd_order_lists_dir)
            {
                dialog = dialog.set_directory(dir);
            }

            let task = self.executor.spawn(async move {
                let files = dialog.pick_files().await;
                let results: Vec<anyhow::Result<_>> = stream::iter(files.into_iter().flatten())
                    .then(async |file| {
                        let contents = file.read().await;
                        let contents =
                            String::from_utf8(contents).context("file contains invalid utf-8")?;
                        let lists = read_order_lists_from_str(&contents)?;
                        anyhow::Ok(stream::iter(lists.into_iter().map(
                            move |(subpaths, order_list)| {
                                let mut path = file.path().to_path_buf();
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
                ctx.request_repaint();
                results
            });
            self.load_task = Some(task);
        }

        let results = if let Some(task) = self.load_task.take() {
            self.executor.try_tick();
            if task.is_finished() {
                Some(block_on(task))
            } else {
                self.load_task = Some(task);
                None
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
        let path = persist_path()?;
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(PersistData::default()),
            Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
        };
        serde_json::from_slice(&contents).with_context(|| format!("parse {}", path.display()))
    }

    fn write_persist_data(&self, data: &PersistData) -> anyhow::Result<()> {
        let path = persist_path()?;
        let dir = path.parent().context("persist file has no parent directory")?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let contents = serde_json::to_vec(data).context("serialize persist data")?;
        fs::write(&path, contents).with_context(|| format!("write {}", path.display()))
    }
}

fn persist_path() -> anyhow::Result<PathBuf> {
    let mut path = dirs::data_dir().context("could not find the user data directory")?;
    path.push("ttd-scheduler");
    path.push("persist.json");
    Ok(path)
}

fn openttd_order_lists_dir() -> Option<PathBuf> {
    let mut path = dirs::data_dir()?;
    path.push("openttd");
    path.push("orderlist");
    Some(path)
}
