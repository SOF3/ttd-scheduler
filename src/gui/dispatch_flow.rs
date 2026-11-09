use std::mem;
use std::path::PathBuf;

use arrayvec::ArrayVec;
use egui::{
    Align2, Color32, FontFamily, FontId, Painter, Popup, PopupKind, Pos2, Rect, ScrollArea, Slider,
    Stroke, Vec2, pos2, vec2,
};

use crate::dispatch_flow::{self, DisplayInfo};
use crate::gui::number_edit;
use crate::schema::{Duration, OrderList, OrderTyped, Ticks, Timestamp};

const ORDER_WRAP_THRESHOLD: f32 = 0.7;

#[derive(Default)]
pub struct State {
    setup:         dispatch_flow::Args,
    result:        Option<DispatchFlowCache>,
    initial_texts: Vec<String>,
    plot:          PlotConfig,
    hovered:       HoveredPair,
}

impl State {
    pub fn ui(&mut self, ui: &mut egui::Ui, (path, order_list): &(PathBuf, Box<OrderList>)) {
        self.hovered.prev = mem::take(&mut self.hovered.next);

        let cache_key = DispatchFlowCacheKey { path: path.clone(), setup: self.setup.clone() };
        let mut force_refresh = false;
        ui.horizontal(|ui| {
            ui.label("Starting vehicles:");
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

                        number_edit(
                            ui,
                            &mut self.initial_texts[sched_index],
                            &mut self.setup.initial.0[sched_index],
                            Some(0),
                            Some(1000),
                        );
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
            ui.checkbox(&mut self.plot.stringline, "Stringline");
            ui.checkbox(&mut self.plot.capacity, "Capacity chart");
        });

        self.plot.ui(ui, order_list, display);

        type PlotFn<'a> = Box<dyn Fn(&mut egui::Ui, &mut PlotConfig, &mut HoveredPair) + 'a>;
        let plots: Vec<PlotFn> = [
            self.plot.stringline.then(|| {
                Box::new(|ui: &mut egui::Ui, plot: &mut PlotConfig, hovered: &mut HoveredPair| {
                    show_stringline(ui, plot, logger, display, order_list, hovered);
                }) as PlotFn
            }),
            self.plot.capacity.then(|| {
                Box::new(|ui: &mut egui::Ui, plot: &mut PlotConfig, hovered: &mut HoveredPair| {
                    show_capacity(ui, &self.setup, logger, plot, display, order_list, hovered);
                }) as PlotFn
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        match &plots[..] {
            [] => {}
            [p1] => p1(ui, &mut self.plot, &mut self.hovered),
            [p1, p2] => ui.columns_const(|[ui1, ui2]| {
                p1(ui1, &mut self.plot, &mut self.hovered);
                p2(ui2, &mut self.plot, &mut self.hovered);
            }),
            _ => unreachable!("we don't hae so many plots"),
        }
    }
}

fn show_stringline(
    ui: &mut egui::Ui,
    plot_config: &PlotConfig,
    logger: &Logger,
    display: &DisplayInfo,
    order_list: &OrderList,
    hovered: &mut HoveredPair,
) {
    let avbl_size = ui.available_size();
    let resolution =
        plot_config.resolution(avbl_size, display, order_list, ui.next_widget_position());
    let (resp, painter) = ui.allocate_painter(avbl_size, egui::Sense::click());
    let painter = painter.with_clip_rect(resp.rect.intersect(painter.clip_rect()));
    painter.rect_filled(resp.rect, 1.0, Color32::from_rgb(60, 80, 50));

    draw_hovered_crosshair(&painter, &resolution, &hovered.prev);

    let vehicle_step_coord = |time: Ticks| {
        move |v: &VehicleStepLog| {
            let order = match v.lerp_last_order {
                Some((prev, lerp)) => {
                    let cycle = resolution.num_orders as f32;
                    let change = v.current_order as f32 - prev as f32;
                    let change = if change > cycle * ORDER_WRAP_THRESHOLD {
                        change - cycle
                    } else if change < -cycle * ORDER_WRAP_THRESHOLD {
                        change + cycle
                    } else {
                        change
                    };
                    (prev as f32 + lerp * change + 0.5).rem_euclid(cycle) - 0.5
                }
                None => v.current_order as f32,
            };
            resolution.tick_order_to_pos(time, order)
        }
    };

    for slot in &logger.missed_slots {
        let Some(order) = order_list
            .orders
            .iter()
            .position(|order| order.schedule_index == Some(slot.schedule_index))
        else {
            continue;
        };
        let pos = resolution.tick_order_to_pos(slot.time, order as f32);
        painter.text(
            pos,
            Align2::CENTER_CENTER,
            "X",
            FontId::new(12.0, FontFamily::Proportional),
            ui.visuals().error_fg_color,
        );
    }

    let mut steps = logger.steps.iter();
    let Some(first_step) = steps.next() else { return };
    let mut vehicle_coords: Vec<_> =
        first_step.vehicles.iter().map(vehicle_step_coord(first_step.time)).collect();

    let mut nearest_hovered_segment = None;

    for step in steps {
        for ((veh_index, veh_step), coords) in
            step.vehicles.iter().enumerate().zip(&mut vehicle_coords)
        {
            let next_coords = vehicle_step_coord(step.time)(veh_step);
            let prev_coords = mem::replace(coords, next_coords);

            let mut stroke = Stroke::new(1.0, Color32::from_rgb(180, 180, 220));
            if let Some(hv) = hovered.prev.vehicle
                && hv.vehicle == veh_index
            {
                stroke.color = Color32::from_rgb(220, 240, 80);
            }

            let cycle = resolution.num_orders as f32 * resolution.px_per_order;
            let offset = plot_config.orientation.vector(0.0, cycle);
            let segments: ArrayVec<[Pos2; 2], 2> =
                if plot_config.orientation.order(next_coords - prev_coords)
                    > cycle * ORDER_WRAP_THRESHOLD
                {
                    [[prev_coords, next_coords - offset], [prev_coords + offset, next_coords]]
                        .into_iter()
                        .collect()
                } else if plot_config.orientation.order(prev_coords - next_coords)
                    > cycle * ORDER_WRAP_THRESHOLD
                {
                    [[prev_coords, next_coords + offset], [prev_coords - offset, next_coords]]
                        .into_iter()
                        .collect()
                } else {
                    [[prev_coords, next_coords]].into_iter().collect()
                };

            for [prev_coords, next_coords] in segments {
                painter.line_segment([prev_coords, next_coords], stroke);

                if let Some(hover) = resp.hover_pos()
                    && Rect::from_two_pos(prev_coords, next_coords).expand(10.0).contains(hover)
                {
                    let dist = point_segment_dist_sq(hover, prev_coords, next_coords);
                    if nearest_hovered_segment.is_none_or(|(d, _)| d > dist) {
                        nearest_hovered_segment = Some((
                            dist,
                            HoveredVehicle {
                                vehicle:    veh_index,
                                prev_order: veh_step.lerp_last_order,
                                next_order: veh_step.current_order,
                            },
                        ));
                    }
                }
            }
        }
    }

    if let Some((_, nearest)) = nearest_hovered_segment {
        hovered.next.vehicle = Some(nearest);
    }

    detect_hover(&resp, hovered, &resolution);
    show_tooltip(
        &resp,
        &hovered.prev,
        order_list,
        display,
        nearest_hovered_segment.is_some(),
        |ui: &mut egui::Ui| {
            let Some((_, vehicle)) = nearest_hovered_segment else { return };
            let next = order_list
                .orders
                .get(vehicle.next_order)
                .and_then(|o| o.typed.destination_name())
                .unwrap_or("Unknown");
            match vehicle.prev_order {
                None => {
                    ui.label(format!("Waiting at {next}"));
                }
                Some((prev, lerp)) => {
                    let prev = order_list
                        .orders
                        .get(prev)
                        .and_then(|o| o.typed.destination_name())
                        .unwrap_or("Unknown");
                    ui.label(format!("Traveling {:.0}% from {prev} to {next}", lerp * 100.0));
                }
            }
        },
    );
}

fn show_capacity(
    ui: &mut egui::Ui,
    _setup: &dispatch_flow::Args,
    logger: &Logger,
    plot_config: &PlotConfig,
    display: &DisplayInfo,
    order_list: &OrderList,
    hovered: &mut HoveredPair,
) {
    let avbl_size = ui.available_size();
    let resolution =
        plot_config.resolution(avbl_size, display, order_list, ui.next_widget_position());
    let (resp, painter) = ui.allocate_painter(avbl_size, egui::Sense::click());
    painter.rect_filled(resp.rect, 1.0, Color32::from_rgb(60, 80, 50));

    let max_waiting = logger
        .steps
        .iter()
        .flat_map(|step| &step.orders)
        .map(|order| order.waiting)
        .max()
        .unwrap_or(1)
        .max(1);

    let mut hovered_waiting = None;
    for (step, end_time) in logger
        .steps
        .iter()
        .zip(logger.steps.iter().skip(1).map(|s| s.time).chain([display.end_time]))
    {
        for (order_index, order) in order_list.orders.iter().enumerate() {
            if let OrderTyped::GoToStation { .. } = order.typed {
                let start_coords =
                    resolution.tick_order_to_pos(step.time, order_index as f32 - 0.5);
                let end_coords =
                    resolution.tick_order_to_pos(end_time.max(step.time), order_index as f32 + 0.5);
                let waiting = step.orders.get(order_index).map(|s| s.waiting).unwrap_or(0);
                let lerp = waiting as f32 / max_waiting as f32;
                let color = if lerp < 0.5 {
                    lerp_rgb(Color32::from_rgb(0, 0, 255), Color32::from_rgb(0, 255, 0), lerp * 2.0)
                } else {
                    lerp_rgb(
                        Color32::from_rgb(0, 255, 0),
                        Color32::from_rgb(255, 0, 0),
                        (lerp - 0.5) * 2.0,
                    )
                };
                let rect = Rect::from_two_pos(start_coords, end_coords);
                painter.rect_filled(rect, 0.0, color);
                if let Some(hover_pos) = resp.hover_pos()
                    && rect.contains(hover_pos)
                {
                    let max_waiting_in_order = logger
                        .steps
                        .iter()
                        .filter_map(|step| step.orders.get(order_index))
                        .map(|order| order.waiting)
                        .sum::<u32>();
                    hovered_waiting = Some((waiting, max_waiting_in_order));
                }
            }
        }
    }

    draw_hovered_crosshair(&painter, &resolution, &hovered.prev);

    detect_hover(&resp, hovered, &resolution);
    show_tooltip(
        &resp,
        &hovered.prev,
        order_list,
        display,
        hovered_waiting.is_some(),
        |ui: &mut egui::Ui| {
            if let Some((waiting, max_waiting_in_order)) = hovered_waiting {
                ui.label(format!("Waiting: {waiting} / {max_waiting_in_order}"));
            }
        },
    );
}

#[derive(PartialEq, Eq)]
struct DispatchFlowCacheKey {
    path:  PathBuf,
    setup: dispatch_flow::Args,
}

struct DispatchFlowCache {
    cache_key: DispatchFlowCacheKey,
    result:    Result<(Logger, DisplayInfo), String>,
}

#[derive(Default)]
struct Logger {
    steps:        Vec<StepLog>,
    missed_slots: Vec<MissedSlot>,
}

struct StepLog {
    time:     Ticks,
    vehicles: Vec<VehicleStepLog>,
    orders:   Vec<OrderStepLog>,
}

struct VehicleStepLog {
    current_order:   usize,
    lerp_last_order: Option<(usize, f32)>,
}

#[derive(Clone)]
struct OrderStepLog {
    waiting: u32,
}

struct MissedSlot {
    schedule_index: usize,
    time:           Ticks,
}

impl dispatch_flow::Logger for Logger {
    fn step(&mut self, order_list: &OrderList, time: Ticks, state: &dispatch_flow::State) {
        let mut orders = vec![OrderStepLog { waiting: 0 }; order_list.orders.len()];
        for vehicle in &state.vehicles {
            let waiting = match vehicle.action {
                dispatch_flow::VehicleAction::PendingDispatch => true,
                dispatch_flow::VehicleAction::Traveling { .. } => false,
                dispatch_flow::VehicleAction::Waiting => true,
            };
            if waiting {
                orders[vehicle.current_order].waiting += 1;
            }
        }

        let vehicles = state
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
            .collect();

        self.steps.push(StepLog { time, vehicles, orders });
    }

    fn missed_dispatch_slot(&mut self, schedule_index: usize, _slot: usize, time: Ticks) {
        self.missed_slots.push(MissedSlot { schedule_index, time });
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
}

#[derive(Debug, Clone, Copy)]
struct PlotConfig {
    stringline: bool,
    capacity:   bool,

    orientation:  Orientation,
    time_scale:   f32,
    time_offset:  Ticks,
    order_scale:  f32,
    order_offset: f32,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            stringline: true,
            capacity:   false,

            orientation:  Orientation::HorizontalTime,
            time_scale:   1.0,
            time_offset:  Ticks(0),
            order_scale:  1.0,
            order_offset: 0.0,
        }
    }
}

impl PlotConfig {
    fn ui(&mut self, ui: &mut egui::Ui, order_list: &OrderList, display: &DisplayInfo) {
        ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                match self.orientation {
                    Orientation::VerticalTime => {
                        if ui.button("Time / Order").clicked() {
                            self.orientation = Orientation::HorizontalTime;
                        }
                    }
                    Orientation::HorizontalTime => {
                        if ui.button("Order / Time").clicked() {
                            self.orientation = Orientation::VerticalTime;
                        }
                    }
                }

                ui.label("Time scale");
                ui.add(Slider::new(&mut self.time_scale, 1.0..=10.0).suffix("x").logarithmic(true));

                ui.label("Time offset");
                let hidden_length = ((display.end_time - display.start_time).0 as f32
                    * (1.0 - self.time_scale.recip())) as i32;
                let offset_duration = Duration::from_ticks(self.time_offset, display.tpm);
                ui.add(Slider::new(&mut self.time_offset.0, 0..=hidden_length).suffix({
                    use std::fmt::Write;
                    let mut suffix = String::from("t");
                    if let Ok(offset_duration) = offset_duration {
                        write!(suffix, " ({offset_duration})").unwrap();
                    }
                    suffix
                }));

                ui.label("Order scale");
                ui.add(Slider::new(&mut self.order_scale, 1.0..=10.0).suffix("x"));

                ui.label("Order offset");
                let hidden_length =
                    order_list.orders.len() as f32 * (1.0 - self.order_scale.recip());
                ui.add(Slider::new(&mut self.order_offset, 0.0..=hidden_length).suffix(""));
            });
        });
    }

    fn resolution(
        &self,
        avbl_size: Vec2,
        display: &DisplayInfo,
        order_list: &OrderList,
        rect_min: Pos2,
    ) -> Resolution {
        let num_orders = order_list.orders.len();
        let px_per_tick = (self.orientation.time(avbl_size).ceil() * self.time_scale)
            / ((display.end_time - display.start_time).0 as f32);
        let px_per_order =
            self.orientation.order(avbl_size) / (num_orders as f32) * self.order_scale;
        Resolution {
            px_per_tick,
            px_per_order,
            config: *self,
            time_range: [display.start_time, display.end_time],
            num_orders: order_list.orders.len(),
            rect: Rect::from_min_size(rect_min, avbl_size),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Resolution {
    px_per_tick:  f32,
    px_per_order: f32,
    config:       PlotConfig,
    time_range:   [Ticks; 2],
    num_orders:   usize,
    rect:         Rect,
}

impl Resolution {
    fn tick_to_pos(&self, ticks: Ticks) -> f32 {
        let tick_px =
            ((ticks - self.time_range[0] - self.config.time_offset).0 as f32) * self.px_per_tick;
        self.config.orientation.time(self.rect.min.to_vec2()) + tick_px
    }

    fn order_to_pos(&self, order: f32) -> f32 {
        let order_px = (order + 0.5 - self.config.order_offset) * self.px_per_order;
        self.config.orientation.order(self.rect.min.to_vec2()) + order_px
    }

    fn tick_order_to_pos(&self, tick: Ticks, order: f32) -> Pos2 {
        match self.config.orientation {
            Orientation::HorizontalTime => pos2(self.tick_to_pos(tick), self.order_to_pos(order)),
            Orientation::VerticalTime => pos2(self.order_to_pos(order), self.tick_to_pos(tick)),
        }
    }

    fn pos_to_tick_order(&self, pos: Pos2) -> (Ticks, f32) {
        let vec = pos - self.rect.min;
        let tick_px = self.config.orientation.time(vec);
        let order_px = self.config.orientation.order(vec);
        let tick = Ticks((tick_px / self.px_per_tick) as i32)
            + self.time_range[0]
            + self.config.time_offset;
        let order = order_px / self.px_per_order + self.config.order_offset;
        (tick, order)
    }

    fn full_tick_line(&self, ticks: Ticks) -> [Pos2; 2] {
        let v = self.tick_to_pos(ticks);
        match self.config.orientation {
            Orientation::HorizontalTime => [pos2(v, self.rect.min.y), pos2(v, self.rect.max.y)],
            Orientation::VerticalTime => [pos2(self.rect.min.x, v), pos2(self.rect.max.x, v)],
        }
    }

    fn full_order_line(&self, order: f32) -> [Pos2; 2] {
        let v = self.order_to_pos(order);
        match self.config.orientation {
            Orientation::HorizontalTime => [pos2(self.rect.min.x, v), pos2(self.rect.max.x, v)],
            Orientation::VerticalTime => [pos2(v, self.rect.min.y), pos2(v, self.rect.max.y)],
        }
    }
}

#[derive(Debug, Default)]
struct HoveredPair {
    prev: Hovered,
    next: Hovered,
}

#[derive(Debug, Clone, Copy, Default)]
struct Hovered {
    order:   Option<usize>,
    time:    Option<Ticks>,
    vehicle: Option<HoveredVehicle>,
}

#[derive(Debug, Clone, Copy)]
struct HoveredVehicle {
    vehicle:    usize,
    prev_order: Option<(usize, f32)>,
    next_order: usize,
}

fn point_segment_dist_sq(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = point - a;
    let ab_len_sq = ab.length_sq();
    if ab_len_sq == 0.0 {
        return ap.length_sq();
    }
    let t = (ap.dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    let projection = a + ab * t;
    (point - projection).length_sq()
}

fn lerp_rgb(a: Color32, b: Color32, t: f32) -> Color32 {
    let (r, g, b, a) = (
        a.r() as f32 + (b.r() as f32 - a.r() as f32) * t,
        a.g() as f32 + (b.g() as f32 - a.g() as f32) * t,
        a.b() as f32 + (b.b() as f32 - a.b() as f32) * t,
        a.a() as f32 + (b.a() as f32 - a.a() as f32) * t,
    );
    Color32::from_rgba_unmultiplied(r as u8, g as u8, b as u8, a as u8)
}

fn draw_hovered_crosshair(painter: &Painter, resolution: &Resolution, hovered: &Hovered) {
    if let Some(ticks) = hovered.time {
        painter.line_segment(
            resolution.full_tick_line(ticks),
            Stroke::new(1.0, Color32::from_rgb(220, 130, 200)),
        );
    }

    if let Some(order) = hovered.order {
        painter.line_segment(
            resolution.full_order_line(order as f32),
            Stroke::new(1.0, Color32::from_rgb(220, 160, 160)),
        );
    }
}

fn detect_hover(resp: &egui::Response, hovered: &mut HoveredPair, resolution: &Resolution) {
    if let Some(hover) = resp.hover_pos() {
        let (time, order) = resolution.pos_to_tick_order(hover);
        let order = order.floor() as usize;

        hovered.next.time = Some(time);
        hovered.next.order = Some(order);
    }
}

fn show_tooltip(
    resp: &egui::Response,
    hovered: &Hovered,
    order_list: &OrderList,
    display: &DisplayInfo,
    force_show: bool,
    additional: impl FnOnce(&mut egui::Ui),
) {
    if !force_show && hovered.time.is_none() && hovered.order.is_none() {
        return;
    }

    Popup::from_response(resp)
        .kind(PopupKind::Tooltip)
        .sense(egui::Sense::empty())
        .at_pointer()
        .show(|ui| {
            ui.set_min_width(200.0);
            if let Some(time) = hovered.time {
                ui.label(format!(
                    "Time: {}",
                    Timestamp::from_ticks(time, display.tpm)
                        .map_or_else(|_| time.0.to_string(), |t| t.to_string())
                ));
            }
            if let Some(order) = hovered.order
                && let Some(order) = order_list.orders.get(order)
                && let OrderTyped::GoToStation { ref destination_name, .. }
                | OrderTyped::GoToWaypoint { ref destination_name, .. } = order.typed
            {
                ui.label(format!("Order: {destination_name}"));
            }

            additional(ui);
        });
}
