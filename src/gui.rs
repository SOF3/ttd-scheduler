use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::num::Wrapping;
use std::ops::{AddAssign, SubAssign};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use egui::{CursorIcon, Label, Margin, ScrollArea, TextEdit};
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

mod charts;
pub mod platform;
pub use platform::Platform;

use crate::dispatch_flow;
use crate::schema::{DestinationId, OrderList};

#[cfg(not(target_arch = "wasm32"))]
#[derive(clap::Args)]
pub struct Command {
    pub files: Vec<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistData {
    chart_setups: HashMap<PathBuf, dispatch_flow::Args>,
    dest_order:   IndexSet<DestinationId>,
}

#[derive(Default)]
struct Persist {
    data:    PersistData,
    changed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Command {
    pub fn run(&self) -> anyhow::Result<()> {
        use anyhow::Context;

        fn read_file(states: &mut States, file: &PathBuf) -> anyhow::Result<()> {
            let contents = std::fs::read_to_string(file).context("read order lists file")?;
            for (subpaths, order_list) in crate::schema::read_order_lists_from_str(&contents)
                .context("parse order list file")?
            {
                let mut path = file.clone();
                for subpath in subpaths {
                    path.push(subpath);
                }
                states.files.files.push(LoadedFile {
                    path,
                    order_list,
                    file_id: FileId(
                        states
                            .files
                            .next_file_id
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    ),
                });
            }
            Ok(())
        }

        let mut app = App::new(platform::Native::default());
        for file in &self.files {
            read_file(&mut app.states, file).context(file.display().to_string())?;
        }

        eframe::run_native(
            "OpenTTD scheduler",
            eframe::NativeOptions::default(),
            Box::new(|_cc| Ok(Box::new(app))),
        )
        .context("eframe")
    }
}

pub struct App {
    platform: Box<dyn Platform>,
    states:   States,

    fps_counter: VecDeque<f64>,
}

impl App {
    pub fn new(platform: impl Platform + 'static) -> Self {
        let mut app = Self {
            platform:    Box::new(platform),
            states:      States::default(),
            fps_counter: VecDeque::new(),
        };
        match app.platform.read_persist_data() {
            Ok(data) => app.states.persist.data = data,
            Err(err) => log::error!("read persisted settings: {err:?}"),
        }
        app
    }
}

#[derive(Default)]
struct States {
    files:   FileList,
    charts:  charts::State,
    persist: Persist,
}

#[derive(Default)]
pub struct FileList {
    files:        Vec<LoadedFile>,
    last_err:     String,
    generation:   Wrapping<u8>,
    next_file_id: Arc<AtomicUsize>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Some(pop) = self.fps_counter.front()
            && ui.time() - pop > 1.0
        {
            self.fps_counter.pop_front();
        }
        self.fps_counter.push_back(ui.time());

        if cfg!(feature = "render-fps") {
            let fps =
                self.fps_counter.len() as f64 / (ui.time() - self.fps_counter.front().unwrap());
            ui.add(Label::new(format!("FPS: {fps}")));
        }

        egui::Panel::top("global top")
            .resizable(true)
            .frame(egui::Frame::new().outer_margin(Margin::same(10)))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("global top scroll")
                    .max_width(ui.content_rect().width())
                    .show(ui, |ui| {
                        self.choose_files(ui);
                        self.states.charts.render_header(
                            ui,
                            &self.states.files,
                            &mut self.states.persist,
                        );
                    });
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.states.charts.render_plots(ui, &self.states.files, &mut self.states.persist);
        });

        if self.states.persist.changed {
            match self.platform.write_persist_data(&self.states.persist.data) {
                Ok(()) => self.states.persist.changed = false,
                Err(err) => log::error!("write persisted settings: {err:?}"),
            }
        }
    }
}

impl App {
    fn choose_files(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut removals = Vec::new();
            for (i, file) in self.states.files.files.iter().enumerate() {
                if ui
                    .colored_label(egui::Color32::RED, "X")
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    removals.push(i);
                }
                ui.label(file.filename());
            }

            if !removals.is_empty() {
                let mut i = 0;
                self.states.files.files.retain(|_| {
                    let remove = removals.contains(&i);
                    i += 1;
                    !remove
                });
                self.states.files.generation += 1;
            }

            let resp = ui.button("Add orderlist file");
            if !self.states.files.last_err.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.states.files.last_err);
            }
            self.platform.choose_order_list_file(&resp.ctx, &mut self.states.files, resp.clicked());
        });
    }
}

struct LoadedFile {
    path:       PathBuf,
    order_list: Box<OrderList>,
    file_id:    FileId,
}

impl LoadedFile {
    fn filename(&self) -> Cow<'_, str> {
        self.path.file_name().expect("regular file must have a name").to_string_lossy()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FileId(usize);

fn number_edit<N>(
    ui: &mut egui::Ui,
    string: &mut String,
    value: &mut N,
    min: Option<N>,
    max: Option<N>,
) where
    N: FromStr + Display + Copy + num_traits::Num + AddAssign + SubAssign + PartialOrd,
{
    ui.scope(|ui| {
        if !string.parse::<N>().is_ok() {
            let visuals = &mut ui.style_mut().visuals;
            visuals.text_edit_bg_color = Some(visuals.error_fg_color);
        }

        let resp = TextEdit::singleline(string).desired_width(40.0).show(ui);

        if let Ok(parsed) = string.parse::<N>() {
            *value = parsed;
        }

        let mut changed = false;
        if resp.response.has_focus() {
            ui.input_mut(|input| {
                for _ in 0..input.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    if let Some(max) = max
                        && *value < max
                    {
                        *value += N::one();
                        changed = true;
                    }
                }
                for _ in 0..input.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                {
                    if let Some(min) = min
                        && *value > min
                    {
                        *value -= N::one();
                        changed = true;
                    }
                }
            });
        }

        if resp.response.lost_focus() || changed {
            *string = value.to_string();
        }
    });
}
