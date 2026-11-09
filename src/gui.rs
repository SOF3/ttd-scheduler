use std::path::PathBuf;

use anyhow::Context;
use egui::Margin;

mod dispatch_flow;
mod platform;
use platform::Platform;

use crate::schema::{OrderList, read_order_list};

#[derive(clap::Args)]
pub struct Command {
    pub file: Option<PathBuf>,
}

impl Command {
    pub fn run(&self) -> anyhow::Result<()> {
        let mut app = App { platform: Box::new(platform::Native), states: States::default() };
        if let Some(file) = &self.file {
            app.states.file = Some((
                file.clone(),
                Box::new(read_order_list(file).context("read order list file")?),
            ));
        }
        eframe::run_native(
            "OpenTTD scheduler",
            eframe::NativeOptions::default(),
            Box::new(|_cc| Ok(Box::new(app))),
        )
        .context("eframe")
    }
}

struct App {
    platform: Box<dyn Platform>,
    states:   States,
}

#[derive(Default)]
struct States {
    file:          Option<(PathBuf, Box<OrderList>)>,
    file_err:      String,
    tab:           WhichTab,
    dispatch_flow: dispatch_flow::State,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WhichTab {
    #[default]
    DispatchFlow,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::new().outer_margin(Margin::same(10)).show(ui, |ui| {
            self.choose_file(ui);

            ui.horizontal(|ui| {
                ui.style_mut().spacing.item_spacing.x = 0.0;

                ui.selectable_value(
                    &mut self.states.tab,
                    WhichTab::DispatchFlow,
                    "Dispatch simulator",
                );
            });
            ui.separator();

            match self.states.tab {
                WhichTab::DispatchFlow => {
                    if let Some(file) = &self.states.file {
                        self.states.dispatch_flow.ui(ui, file)
                    }
                }
            }
        });
    }
}

impl App {
    fn choose_file(&mut self, ui: &mut egui::Ui) {
        let choose_file = ui.horizontal(|ui| {
            let resp = ui.button("Choose file");
            if let Some((file, _)) = &self.states.file {
                ui.label(file.display().to_string());
            }
            if !self.states.file_err.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.states.file_err);
            }
            resp.clicked()
        });
        if choose_file.inner {
            self.platform.choose_order_list_file(
                &mut self.states.file,
                &mut self.states.file_err,
                choose_file.inner,
            );
        }
    }
}
