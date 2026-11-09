use std::collections::VecDeque;
use std::fmt::Display;
use std::ops::{AddAssign, SubAssign};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context;
use egui::{Label, Margin, TextEdit};

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
        let mut app = App {
            platform:    Box::new(platform::Native),
            states:      States::default(),
            fps_counter: VecDeque::new(),
        };
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

    fps_counter: VecDeque<f64>,
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
