use std::mem;
use std::path::PathBuf;

use egui::{Color32, Popup, PopupKind, Slider, Stroke, TextEdit, Vec2, vec2};

use crate::dispatch_flow;
use crate::schema::{OrderList, OrderTyped, Ticks, Timestamp};

#[derive(Default)]
pub struct State {
    setup:         dispatch_flow::Args,
    result:        Option<DispatchFlowCache>,
    initial_texts: Vec<String>,
    plot:          PlotConfig,
}

struct PlotConfig {
    stringline: bool,
    minimap:    bool,

    orientation: Orientation,
    time_scale:  f32,
    order_scale: f32,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            stringline: true,
            minimap:    false,

            orientation: Orientation::VerticalTime,
            time_scale:  1.0,
            order_scale: 1.0,
        }
    }
}

impl State {
    pub fn ui(&mut self, ui: &mut egui::Ui, (path, order_list): &(PathBuf, Box<OrderList>)) {
        let cache_key = DispatchFlowCacheKey { path: path.clone(), setup: self.setup.clone() };
        let mut force_refresh = false;
        ui.horizontal(|ui| {
            ui.label("Starting vehicles at:");
            self.initial_texts.resize(order_list.schedules.len(), String::from("0"));
            self.setup.initial.0.resize(order_list.schedules.len(), 0);
            ui.vertical(|ui| {
                for sched_index in 0..order_list.schedules.len() {
                    let order =
                        order_list.orders.iter().find(|o| o.schedule_index == Some(sched_index));
                    let name = match order.map(|o| &o.typed) {
                        Some(OrderTyped::GoToStation { destination_name, .. }) => {
                            destination_name.as_str()
                        }
                        Some(OrderTyped::GoToWaypoint { destination_name, .. }) => {
                            destination_name.as_str()
                        }
                        _ => "(unassigned)",
                    };
                    ui.horizontal(|ui| {
                        ui.label(format!("Schedule {} ({name})", sched_index + 1));

                        let text = &mut self.initial_texts[sched_index];
                        if text.parse::<u32>().is_err() {
                            ui.style_mut().visuals.text_edit_bg_color = Some(Color32::DARK_RED);
                        }
                        ui.add(TextEdit::singleline(text).desired_width(50.0));
                        if let Ok(number) = text.parse::<u32>() {
                            self.setup.initial.0[sched_index] = number;
                        }
                    });
                }
            });
            force_refresh = ui.button("Refresh").clicked();
        });
        let result = match &self.result {
            Some(result) if force_refresh || result.cache_key != cache_key => None,
            None => None,
            Some(result) => Some(result),
        };
        let result = match result {
            Some(result) => result,
            None => {
                let mut logger = Logger::default();
                let cache = match self.setup.simulate(order_list, &mut logger) {
                    Ok(display) => DispatchFlowCache { cache_key, result: Ok((logger, display)) },
                    Err(err) => DispatchFlowCache { cache_key, result: Err(format!("{err:?}")) },
                };
                self.result.insert(cache)
            }
        };

        let (logger, display) = match &result.result {
            Ok(data) => data,
            Err(err) => {
                ui.colored_label(egui::Color32::RED, err);
                return;
            }
        };
        ui.horizontal(|ui| {
            ui.style_mut().spacing.item_spacing.x = 0.0;

            ui.checkbox(&mut self.plot.stringline, "Stringline");
        });

        type PlotFn<'a> = Box<dyn Fn(&mut egui::Ui, &mut PlotConfig) + 'a>;
        let plots: Vec<PlotFn> = [
            self.plot.stringline.then(|| Box::new(|ui: &mut egui::Ui, plot: &mut PlotConfig| {
                show_stringline(ui, plot, &logger, display, order_list);
            }) as PlotFn),
        ].into_iter().flatten().collect();
        match &plots[..] {
            [] => {}
            [p1] => p1(ui, &mut self.plot),
            [p1, p2] => ui.columns_const(|[ui1, ui2]| {
                p1(ui1, &mut self.plot);
                p2(ui2, &mut self.plot);
            }),
            _ => unreachable!("we don't hae so many plots"),
        }
    }
}

fn show_stringline(
    ui: &mut egui::Ui,
    heatmap: &mut PlotConfig,
    logger: &Logger,
    display: &dispatch_flow::DisplayInfo,
    order_list: &OrderList,
) {
    ui.horizontal(|ui| {
        match heatmap.orientation {
            Orientation::VerticalTime => {
                if ui.button("Time / Order").clicked() {
                    heatmap.orientation = Orientation::HorizontalTime;
                }
            }
            Orientation::HorizontalTime => {
                if ui.button("Order / Time").clicked() {
                    heatmap.orientation = Orientation::VerticalTime;
                }
            }
        }

        ui.label("Time scale");
        ui.add(Slider::new(&mut heatmap.time_scale, 1.0..=10.0).suffix("x"));
        ui.label("Order scale");
        ui.add(Slider::new(&mut heatmap.order_scale, 1.0..=10.0).suffix("x"));
    });

    let max_size = ui.available_size();
    let px_per_tick = (heatmap.orientation.time(max_size).ceil() * heatmap.time_scale)
        / ((display.end_time - display.start_time).0 as f32);
    let px_per_order = heatmap.orientation.order(max_size) / (order_list.orders.len() as f32)
        * heatmap.order_scale;
    let rect_size = heatmap.orientation.vector(
        px_per_tick * (display.end_time - display.start_time).0 as f32,
        px_per_order * order_list.orders.len() as f32,
    );
    let (resp, painter) = ui.allocate_painter(rect_size, egui::Sense::hover());
    painter.rect_filled(resp.rect, 1.0, Color32::from_rgb(60, 80, 60));

    let tick_order_coord = |tick: Ticks, order: f32| {
        let tick_px = (tick - display.start_time).0 as f32 * px_per_tick;
        let order_px = (order + 0.5) * px_per_order;
        resp.rect.min + heatmap.orientation.vector(tick_px, order_px)
    };
    let vehicle_step_coord = |time: Ticks| {
        move |v: &VehicleStepLog| {
            let order = match v.lerp_last_order {
                Some((prev, lerp)) => prev as f32 + lerp * (v.current_order as f32 - prev as f32),
                None => v.current_order as f32,
            };
            tick_order_coord(time, order)
        }
    };

    let mut steps = logger.steps.iter();
    let Some(first_step) = steps.next() else { return };
    let mut vehicle_coords: Vec<_> =
        first_step.vehicles.iter().map(vehicle_step_coord(first_step.time)).collect();

    for step in steps {
        for (v, coords) in step.vehicles.iter().zip(&mut vehicle_coords) {
            let next_coords = vehicle_step_coord(step.time)(v);
            let prev_coords = mem::replace(coords, next_coords);
            painter.line_segment(
                [prev_coords, next_coords],
                Stroke::new(1.0, Color32::from_rgb(200, 200, 200)),
            );
        }
    }

    if let Some(hover) = resp.hover_pos() {
        let (time_px, order_px) = heatmap.orientation.extract_time_order(hover - resp.rect.min);
        let time = Ticks((time_px / px_per_tick) as u32);
        let order = (order_px / px_per_order).floor() as usize;
        Popup::from_response(&resp).kind(PopupKind::Tooltip).at_position(hover + vec2(10.0, 10.0)).show(|ui| {
            ui.label(format!(
                "Time: {}",
                Timestamp::from_ticks(time, display.tpm)
                    .map_or_else(|_| time.0.to_string(), |t| t.to_string())
            ));
            if let Some(order) = order_list.orders.get(order)
                && let OrderTyped::GoToStation { ref destination_name, .. }
                | OrderTyped::GoToWaypoint { ref destination_name, .. } = order.typed
            {
                ui.label(format!("Order: {destination_name}"));
            }
        });
    }
}

#[derive(PartialEq, Eq)]
struct DispatchFlowCacheKey {
    path:  PathBuf,
    setup: dispatch_flow::Args,
}

struct DispatchFlowCache {
    cache_key: DispatchFlowCacheKey,
    result:    Result<(Logger, dispatch_flow::DisplayInfo), String>,
}

#[derive(Default)]
struct Logger {
    steps: Vec<StepLog>,
}

struct StepLog {
    time:     Ticks,
    vehicles: Vec<VehicleStepLog>,
}

struct VehicleStepLog {
    current_order:   usize,
    lerp_last_order: Option<(usize, f32)>,
}

impl dispatch_flow::Logger for Logger {
    fn step(&mut self, _order_list: &OrderList, time: Ticks, state: &dispatch_flow::State) {
        self.steps.push(StepLog {
            time,
            vehicles: state
                .vehicles
                .iter()
                .map(|vehicle| VehicleStepLog {
                    current_order:   vehicle.current_order,
                    lerp_last_order: vehicle.last_order.and_then(|(order, start)| {
                        match vehicle.action {
                            dispatch_flow::VehicleAction::Traveling { eta } => {
                                let lerp = (time - start).0 as f32 / (eta - start).0 as f32;
                                Some((order, lerp))
                            }
                            _ => None,
                        }
                    }),
                })
                .collect(),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Orientation {
    HorizontalTime,
    VerticalTime,
}

impl Orientation {
    fn time(self, vec: Vec2) -> f32 {
        match self {
            Orientation::HorizontalTime => vec.x,
            Orientation::VerticalTime => vec.y,
        }
    }

    fn order(self, vec: Vec2) -> f32 {
        match self {
            Orientation::HorizontalTime => vec.y,
            Orientation::VerticalTime => vec.x,
        }
    }

    fn vector(self, time: f32, order: f32) -> Vec2 {
        match self {
            Orientation::HorizontalTime => vec2(time, order),
            Orientation::VerticalTime => vec2(order, time),
        }
    }

    fn extract_time_order(self, vec: Vec2) -> (f32, f32) {
        match self {
            Orientation::HorizontalTime => (vec.x, vec.y),
            Orientation::VerticalTime => (vec.y, vec.x),
        }
    }
}
