use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::f32::consts::GOLDEN_RATIO;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::Wrapping;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fmt, mem};

use arrayvec::ArrayVec;
use egui::epaint::Hsva;
use egui::text::LayoutJob;
use egui::{
    Align2, Color32, ComboBox, FontFamily, FontId, Painter, Popup, PopupKind, Pos2, Rect,
    ScrollArea, SetOpenCommand, Slider, Stroke, Vec2, pos2, vec2,
};
use indexmap::IndexSet;
use ordered_float::OrderedFloat;

use crate::dispatch_flow::{self, DisplayInfo, VehicleList};
use crate::gui::{FileId, FileList, Persist, number_edit};
use crate::schema::{
    self, DestinationId, Duration, OrderList, OrderTyped, Route, Ticks, Timestamp,
};

const ORDER_WRAP_THRESHOLD: f32 = 0.7;

#[derive(Default)]
pub struct State {
    setups:      HashMap<FileId, dispatch_flow::Args>,
    prev_setups: HashMap<PathBuf, FileId>,
    cache:       Option<SimulationCache>,

    initial_texts: HashMap<(FileId, usize), String>,
    extra_slot:    ExtraSlotState,

    plot:    PlotConfig,
    hovered: HoveredPair,
    palette: Palette,
}

#[derive(Default)]
struct ExtraSlotState {
    add_hour_text:   String,
    add_hour_num:    u8,
    add_minute_text: String,
    add_minute_num:  u8,
    route:           Option<Route>,
}

impl State {
    pub fn ui(&mut self, ui: &mut egui::Ui, loaded: &FileList, persist: &mut Persist) {
        if loaded.files.is_empty() {
            return;
        }

        self.hovered.prev = mem::take(&mut self.hovered.next);

        for file in &loaded.files {
            if !self.setups.contains_key(&file.file_id) {
                let setup = self
                    .prev_setups
                    .get(&file.path)
                    .and_then(|id| self.setups.get(id))
                    .or_else(|| persist.data.chart_setups.get(&file.path))
                    .cloned()
                    .unwrap_or_default();
                self.setups.insert(file.file_id, setup);
                self.prev_setups.insert(file.path.clone(), file.file_id);
            }
            let setup = self.setups.get_mut(&file.file_id).expect("just inserted");
            setup.initial.0.resize(file.order_list.schedules.len(), 0);
        }

        ui.horizontal(|ui| {
            ui.label("Starting vehicles:");

            for (file_index, file) in loaded.files.iter().enumerate() {
                ui.vertical(|ui| {
                    let mut changed = false;
                    for (sched_index, sched) in file.order_list.schedules.iter().enumerate() {
                        let setup = self.setups.get_mut(&file.file_id).expect("inserted above");
                        let initial_num = &mut setup.initial.0[sched_index];
                        let previous = *initial_num;

                        let order = file.order_list.find_schedule_order(sched_index);
                        let name = order
                            .and_then(|(_, order)| order.typed.destination_name())
                            .unwrap_or("(unassigned)");
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} sched#{} ({name})",
                                file.filename(),
                                sched_index + 1
                            ));

                            let text = self
                                .initial_texts
                                .entry((file.file_id, sched_index))
                                .or_insert_with(|| initial_num.to_string());
                            number_edit(ui, text, initial_num, Some(0), Some(1000));

                            let mut delete_extra_slots = Vec::new();
                            for (extra_index, extra_slot) in
                                setup.with_extra_slots.iter().enumerate()
                            {
                                if extra_slot.schedule_index == sched_index {
                                    if ui
                                        .colored_label(Color32::RED, "X")
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        delete_extra_slots.push(extra_index);
                                    }
                                    ui.label(extra_slot.slot_time.to_string()).on_hover_text(
                                        format!(
                                            "Route: {}",
                                            match &extra_slot.route {
                                                None => Cow::Borrowed("Default"),
                                                Some(route) =>
                                                    sched.route_ids.get(route).map_or_else(
                                                        || format!("route {}", route.0).into(),
                                                        |s| Cow::Borrowed(s.as_str())
                                                    ),
                                            }
                                        ),
                                    );
                                }
                            }
                            let mut i = 0;
                            setup.with_extra_slots.retain(|_| {
                                let delete = delete_extra_slots.contains(&i);
                                i += 1;
                                !delete
                            });

                            ui.push_id(("extra slot", file_index, sched_index), |ui| {
                                let add_button = ui.button("Add temp slot");
                                Popup::from_response(&add_button)
                                    .kind(PopupKind::Menu)
                                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                    .open_memory(
                                        add_button.clicked().then_some(SetOpenCommand::Bool(true)),
                                    )
                                    .show(|ui| {
                                        let texts = &mut self.extra_slot;
                                        number_edit(
                                            ui,
                                            &mut texts.add_hour_text,
                                            &mut texts.add_hour_num,
                                            Some(0),
                                            Some(23),
                                        );
                                        number_edit(
                                            ui,
                                            &mut texts.add_minute_text,
                                            &mut texts.add_minute_num,
                                            Some(0),
                                            Some(59),
                                        );
                                        ui.selectable_value(&mut texts.route, None, "Default");
                                        for (&route, route_name) in sched.route_ids.iter() {
                                            ui.selectable_value(
                                                &mut texts.route,
                                                Some(route),
                                                route_name,
                                            );
                                        }
                                        if ui.button("Add").clicked() {
                                            setup.with_extra_slots.push(dispatch_flow::ExtraSlot {
                                                schedule_index: sched_index,
                                                slot_time:      Timestamp {
                                                    hours:   texts.add_hour_num,
                                                    minutes: texts.add_minute_num,
                                                },
                                                route:          texts.route,
                                                tags:           ArrayVec::new(), // TODO
                                            });
                                        }
                                    });
                            });
                        });
                        changed |= *initial_num != previous;
                    }
                    if changed {
                        persist.data.chart_setups.insert(
                            file.path.clone(),
                            self.setups.get(&file.file_id).expect("inserted above").clone(),
                        );
                        persist.changed = true;
                    }
                });
            }
        });
        let result = match &self.cache {
            Some(result)
                if result.cache_key.generation != loaded.generation
                    || result.cache_key.setups != self.setups =>
            {
                None
            }
            None => None,
            Some(result) => Some(result),
        };
        let result = match result {
            Some(result) => result,
            None => {
                let mut display = None::<DisplayInfo>;
                let result: Result<Vec<_>, _> = loaded
                    .files
                    .iter()
                    .map(|file| {
                        let mut logger = Logger::default();
                        let setup = self.setups.get(&file.file_id).expect("inserted above");
                        match setup.simulate(&file.order_list, &mut logger) {
                            Ok(d) => {
                                logger.vehicle_list = d.vehicles.clone();
                                match display {
                                    Some(ref mut display) => {
                                        anyhow::ensure!(
                                            display.tpm == d.tpm,
                                            "ticks-per-minute is inconsistent"
                                        );
                                        display.start_time = display.start_time.min(d.start_time);
                                        display.end_time = display.end_time.max(d.end_time);
                                    }
                                    None => {
                                        display = Some(d);
                                    }
                                }
                                Ok(logger)
                            }
                            Err(err) => Err(err),
                        }
                    })
                    .collect();

                let cache_key = SimulationCacheKey {
                    generation: loaded.generation,
                    setups:     self.setups.clone(),
                };

                let cache = match result {
                    Ok(result) => SimulationCache {
                        cache_key,
                        result: Ok((result, display.expect("loaded.files is not empty"))),
                    },
                    Err(err) => SimulationCache { cache_key, result: Err(format!("{err:?}")) },
                };
                self.plot.dest_permutation.resize(loaded, persist);
                self.cache.insert(cache)
            }
        };

        let (loggers, display) = match &result.result {
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

        self.plot.ui(ui, loaded, display);

        type PlotFn<'a> = Box<
            dyn Fn(&mut egui::Ui, &mut PlotConfig, &mut HoveredPair, &mut Persist, &mut Palette)
                + 'a,
        >;
        let plots: Vec<PlotFn> = [
            self.plot.stringline.then(|| {
                Box::new(
                    |ui: &mut egui::Ui,
                     plot: &mut PlotConfig,
                     hovered: &mut HoveredPair,
                     persist: &mut Persist,
                     palette: &mut Palette| {
                        show_stringline(
                            ui, plot, loggers, display, loaded, hovered, persist, palette,
                        );
                    },
                ) as PlotFn
            }),
            self.plot.capacity.then(|| {
                Box::new(
                    |ui: &mut egui::Ui,
                     plot: &mut PlotConfig,
                     hovered: &mut HoveredPair,
                     persist: &mut Persist,
                     _palette: &mut Palette| {
                        show_capacity(ui, loggers, plot, display, loaded, hovered, persist);
                    },
                ) as PlotFn
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        match &plots[..] {
            [] => {}
            [p1] => p1(ui, &mut self.plot, &mut self.hovered, persist, &mut self.palette),
            [p1, p2] => ui.columns_const(|[ui1, ui2]| {
                p1(ui1, &mut self.plot, &mut self.hovered, persist, &mut self.palette);
                p2(ui2, &mut self.plot, &mut self.hovered, persist, &mut self.palette);
            }),
            _ => unreachable!("we don't hae so many plots"),
        }
    }
}

fn show_stringline(
    ui: &mut egui::Ui,
    plot_config: &mut PlotConfig,
    loggers: &[Logger],
    display: &DisplayInfo,
    loaded: &FileList,
    hovered: &mut HoveredPair,
    persist: &mut Persist,
    palette: &mut Palette,
) {
    let (galleys, galley_margin) = prepare_galleys(ui, loaded, plot_config);

    let avbl_size = ui.available_size();
    let mut paint_size = avbl_size;
    let mut start_pos = ui.next_widget_position();
    match plot_config.orientation {
        Orientation::HorizontalTime => {
            paint_size.x = avbl_size.x - galley_margin;
            start_pos.x += galley_margin;
        }
        Orientation::VerticalTime => {
            paint_size.y = avbl_size.y - galley_margin;
            start_pos.y += galley_margin;
        }
    };
    let resolution = plot_config.resolution(paint_size, display, loaded, start_pos);
    let (resp, painter) = ui.allocate_painter(avbl_size, egui::Sense::click_and_drag());
    let painter = painter.with_clip_rect(resp.rect.intersect(painter.clip_rect()));
    painter.rect_filled(resp.rect, 1.0, Color32::from_rgb(60, 80, 50));

    for (&order, &offset) in &resolution.order_mapping {
        let Some(galley) = order
            .get(loaded)
            .and_then(|order| order.typed.destination_name())
            .and_then(|str| galleys.get(str).cloned())
        else {
            continue;
        };

        let pos = resolution.mapped_order_to_pos(plot_config, offset);
        match plot_config.orientation {
            Orientation::HorizontalTime => {
                painter.galley(pos2(resp.rect.left() + TEXT_PADDING, pos), galley, DEST_TEXT_COLOR);
            }
            Orientation::VerticalTime => {
                painter.galley(pos2(pos, resp.rect.top() + TEXT_PADDING), galley, DEST_TEXT_COLOR);
            }
        }
    }

    draw_hovered_crosshair(&painter, plot_config, &resolution, &hovered.prev);

    draw_time_ticks(&painter, plot_config, &resolution, display);

    let mut nearest_hovered_segment = None;

    for (file_index, (logger, file)) in loggers.iter().zip(&loaded.files).enumerate() {
        let vehicle_step_coord = |time: Ticks| {
            let resolution = &resolution;
            let plot_config = &*plot_config;
            move |v: &VehicleStepLog| {
                let order = match v.lerp_last_order {
                    Some((prev, lerp)) => ResolveOrder::Lerp(
                        OrderRef { file_index, order_index: prev },
                        OrderRef { file_index, order_index: v.current_order },
                        lerp,
                    ),
                    None => {
                        ResolveOrder::Exact(OrderRef { file_index, order_index: v.current_order })
                    }
                };
                resolution.tick_order_to_pos(plot_config, time, order)
            }
        };

        for slot in &logger.missed_slots {
            let Some(order_index) = file
                .order_list
                .orders
                .iter()
                .position(|order| order.schedule_index == Some(slot.schedule_index))
            else {
                continue;
            };
            let pos = resolution.tick_order_to_pos(
                &*plot_config,
                slot.time,
                ResolveOrder::Exact(OrderRef { file_index, order_index }),
            );
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

        for step in steps {
            for ((vehicle_index, veh_step), coords) in
                step.vehicles.iter().enumerate().zip(&mut vehicle_coords)
            {
                let vehicle = VehicleRef { file_index, vehicle_index };
                let next_coords = vehicle_step_coord(step.time)(veh_step);
                let prev_coords = mem::replace(coords, next_coords);

                let mut stroke = Stroke::new(
                    1.0,
                    vehicle_stroke_color(vehicle, plot_config, &logger.vehicle_list, palette),
                );
                if let Some(hv) = hovered.prev.vehicle
                    && hv.vehicle == vehicle
                {
                    stroke.color =
                        Color32::from(Hsva::new(ui.time().fract() as f32, 0.8, 0.8, 1.0));
                    ui.request_repaint();
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
                                    vehicle,
                                    prev_order: veh_step.lerp_last_order,
                                    next_order: veh_step.current_order,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    if let Some((_, nearest)) = nearest_hovered_segment {
        hovered.next.vehicle = Some(nearest);
    }

    detect_hover(&resp, hovered, plot_config, &resolution, persist);
    show_tooltip(
        &resp,
        &hovered.prev,
        loaded,
        display,
        plot_config,
        &resolution,
        nearest_hovered_segment.is_some(),
        |ui: &mut egui::Ui| {
            let Some((_, hovered)) = nearest_hovered_segment else { return };
            let order_list = &*loaded.files[hovered.vehicle.file_index].order_list;
            let next = order_list
                .orders
                .get(hovered.next_order)
                .and_then(|o| o.typed.destination_name())
                .unwrap_or("Unknown");
            match hovered.prev_order {
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

const DEST_TEXT_COLOR: Color32 = Color32::from_rgb(230, 180, 230);
const TEXT_PADDING: f32 = 3.0;

fn prepare_galleys<'f>(
    ui: &mut egui::Ui,
    loaded: &'f FileList,
    plot_config: &PlotConfig,
) -> (HashMap<&'f str, Arc<egui::Galley>>, f32) {
    let galleys = loaded
        .files
        .iter()
        .flat_map(|file| &file.order_list.orders)
        .filter_map(|order| order.typed.destination_name())
        .map(|name| {
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_job(LayoutJob::simple(
                    name.into(),
                    FontId::new(10.0, FontFamily::Proportional),
                    DEST_TEXT_COLOR,
                    120.0,
                ))
            });
            (name, galley)
        })
        .collect::<HashMap<_, _>>();
    let galley_margin = galleys
        .values()
        .map(|galley| plot_config.orientation.time(galley.size()))
        .fold(0.0, f32::max)
        + TEXT_PADDING * 2.0;

    (galleys, galley_margin)
}

fn show_capacity(
    ui: &mut egui::Ui,
    loggers: &[Logger],
    plot_config: &mut PlotConfig,
    display: &DisplayInfo,
    loaded: &FileList,
    hovered: &mut HoveredPair,
    persist: &mut Persist,
) {
    let avbl_size = ui.available_size();
    let resolution = plot_config.resolution(avbl_size, display, loaded, ui.next_widget_position());
    let (resp, painter) = ui.allocate_painter(avbl_size, egui::Sense::click());
    let painter = painter.with_clip_rect(resp.rect.intersect(painter.clip_rect()));
    painter.rect_filled(resp.rect, 1.0, Color32::from_rgb(60, 80, 50));

    let rects: Vec<(Ticks, Ticks, DestOrOrder, u32, u32)> = if plot_config.group_by_dest {
        let mut dest_orders = HashMap::<DestinationId, Vec<_>>::new();
        for (file_index, file) in loaded.files.iter().enumerate() {
            for (order_index, order) in file.order_list.orders.iter().enumerate() {
                if let Some(dest) = order.typed.destination_id() {
                    dest_orders.entry(dest).or_default().push(OrderRef { file_index, order_index });
                }
            }
        }
        let mut step_time: Vec<_> = loggers
            .iter()
            .flat_map(|logger| logger.steps.iter())
            .map(|step| step.time)
            .chain([display.end_time])
            .collect();
        step_time.sort();
        plot_config
            .dest_permutation
            .iter()
            .flat_map(|(_, dest)| {
                let steps: Vec<_> = step_time
                    .iter()
                    .zip(step_time.iter().skip(1))
                    .map(|(&start_time, &end_time)| {
                        let sum: u32 = dest_orders
                            .get(&dest)
                            .into_iter()
                            .flatten()
                            .filter_map(|order| {
                                loggers[order.file_index]
                                    .binary_search_step(start_time)
                                    .and_then(|step| step.orders.get(order.order_index))
                                    .map(|order| order.waiting)
                            })
                            .sum();
                        (start_time, end_time, sum)
                    })
                    .collect();
                let dest_max_waiting =
                    steps.iter().map(|(_, _, waiting)| *waiting).max().unwrap_or(0);
                steps.into_iter().map(move |(start, end, waiting)| {
                    (start, end, DestOrOrder::Dest(dest), waiting, dest_max_waiting)
                })
            })
            .collect()
    } else {
        loaded
            .files
            .iter()
            .enumerate()
            .flat_map(|(file_index, file)| {
                file.order_list
                    .orders
                    .iter()
                    .enumerate()
                    .filter(|(_, order)| matches!(order.typed, OrderTyped::GoToStation { .. }))
                    .flat_map(move |(order_index, _)| {
                        let order = OrderRef { file_index, order_index };
                        let logger = &loggers[file_index];
                        let steps: Vec<_> = logger
                            .steps
                            .iter()
                            .zip(
                                logger
                                    .steps
                                    .iter()
                                    .skip(1)
                                    .map(|s| s.time)
                                    .chain([display.end_time]),
                            )
                            .map(|(step, end_time)| {
                                let waiting =
                                    step.orders.get(order_index).map(|s| s.waiting).unwrap_or(0);
                                (step.time, end_time, waiting)
                            })
                            .collect();
                        let order_max_waiting = steps.iter().map(|(_, _, w)| *w).max().unwrap_or(0);
                        steps.into_iter().map(move |(start, end, waiting)| {
                            (start, end, DestOrOrder::Order(order), waiting, order_max_waiting)
                        })
                    })
            })
            .collect()
    };

    let max_waiting = rects.iter().map(|(_, _, _, waiting, _)| *waiting).max().unwrap_or(1).max(1);

    let mut hovered_waiting = None;
    for (start_time, end_time, order_pos, waiting, row_max_waiting) in rects {
        let start_coords = resolution.tick_order_to_pos(
            &*plot_config,
            start_time.min(end_time),
            ResolveOrder::StartBound(order_pos),
        );
        let end_coords = resolution.tick_order_to_pos(
            &*plot_config,
            end_time.max(start_time),
            ResolveOrder::EndBound(order_pos),
        );
        let lerp = waiting as f32 / max_waiting as f32;
        let color = if lerp < 0.5 {
            lerp_rgb(Color32::from_rgb(0, 0, 255), Color32::from_rgb(0, 255, 0), lerp * 2.0)
        } else {
            lerp_rgb(Color32::from_rgb(0, 255, 0), Color32::from_rgb(255, 0, 0), (lerp - 0.5) * 2.0)
        };
        let rect = Rect::from_two_pos(start_coords, end_coords);
        let cycle = resolution.num_orders as f32 * resolution.px_per_order;
        let offset = plot_config.orientation.vector(0.0, cycle);
        for rect in [rect, rect.translate(-offset)] {
            painter.rect_filled(rect, 0.0, color);
            if let Some(hover_pos) = resp.hover_pos()
                && rect.intersect(resp.rect).contains(hover_pos)
            {
                hovered_waiting = Some((waiting, row_max_waiting));
            }
        }
    }

    draw_hovered_crosshair(&painter, plot_config, &resolution, &hovered.prev);

    detect_hover(&resp, hovered, plot_config, &resolution, persist);
    show_tooltip(
        &resp,
        &hovered.prev,
        loaded,
        display,
        plot_config,
        &resolution,
        hovered_waiting.is_some(),
        |ui: &mut egui::Ui| {
            if let Some((waiting, max_waiting_in_order)) = hovered_waiting {
                ui.label(format!("Waiting: {waiting} / {max_waiting_in_order}"));
            }
        },
    );
}

#[derive(PartialEq, Eq)]
struct SimulationCacheKey {
    generation: Wrapping<u8>,
    setups:     HashMap<FileId, dispatch_flow::Args>,
}

struct SimulationCache {
    cache_key: SimulationCacheKey,
    result:    Result<(Vec<Logger>, DisplayInfo), String>,
}

#[derive(Default)]
struct Logger {
    steps:        Vec<StepLog>,
    missed_slots: Vec<MissedSlot>,
    vehicle_list: VehicleList,
}

impl Logger {
    fn binary_search_step(&self, time: Ticks) -> Option<&StepLog> {
        self.steps
            .partition_point(|step| step.time <= time)
            .checked_sub(1)
            .and_then(|index| self.steps.get(index))
    }
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

#[derive(Debug)]
struct PlotConfig {
    stringline: bool,
    capacity:   bool,

    orientation:        Orientation,
    time_scale:         f32,
    time_offset:        Ticks,
    order_scale:        f32,
    order_offset:       f32,
    time_ticks_density: usize,

    group_by_dest:        bool,
    vehicle_color_scheme: VehicleColorScheme,

    dest_permutation:       DestPermutation,
    override_dest_pos_once: Option<(DestinationId, f32)>,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            stringline: true,
            capacity:   false,

            orientation:        Orientation::HorizontalTime,
            time_scale:         1.0,
            time_offset:        Ticks(0),
            order_scale:        1.0,
            order_offset:       0.0,
            time_ticks_density: 10,

            group_by_dest:        true,
            vehicle_color_scheme: VehicleColorScheme::StartingSchedule,

            dest_permutation:       DestPermutation::default(),
            override_dest_pos_once: None,
        }
    }
}

impl PlotConfig {
    fn ui(&mut self, ui: &mut egui::Ui, loaded: &FileList, display: &DisplayInfo) {
        ScrollArea::horizontal().show(ui, |ui| {
            let (order_term, order_len) = if self.group_by_dest {
                ("Location", self.dest_permutation.len())
            } else {
                ("Order", loaded.files.iter().map(|order| order.order_list.orders.len()).sum())
            };

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.group_by_dest, "Group by destination");

                ComboBox::from_id_salt("VehicleColorScheme")
                    .selected_text(self.vehicle_color_scheme.to_string())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.vehicle_color_scheme,
                            VehicleColorScheme::OrderList,
                            VehicleColorScheme::OrderList.to_string(),
                        );
                        ui.selectable_value(
                            &mut self.vehicle_color_scheme,
                            VehicleColorScheme::StartingSchedule,
                            VehicleColorScheme::StartingSchedule.to_string(),
                        );
                        ui.selectable_value(
                            &mut self.vehicle_color_scheme,
                            VehicleColorScheme::Vehicle,
                            VehicleColorScheme::Vehicle.to_string(),
                        );
                    });

                ui.add(Slider::new(&mut self.time_ticks_density, 1..=30).text("Time axis lines"));

                match self.orientation {
                    Orientation::VerticalTime => {
                        if ui.button(format!("Time / {order_term}")).clicked() {
                            self.orientation = Orientation::HorizontalTime;
                        }
                    }
                    Orientation::HorizontalTime => {
                        if ui.button(format!("{order_term} / Time")).clicked() {
                            self.orientation = Orientation::VerticalTime;
                        }
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("Time scale");
                ui.add(Slider::new(&mut self.time_scale, 0.5..=10.0).suffix("x").logarithmic(true));

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

                ui.label(format!("{order_term} scale"));
                ui.add(Slider::new(&mut self.order_scale, 0.5..=10.0).suffix("x"));

                ui.label(format!("{order_term} offset"));
                ui.add(Slider::new(&mut self.order_offset, 0.0..=order_len as f32).suffix(""));
            });
        });
    }

    fn resolution(
        &mut self,
        avbl_size: Vec2,
        display: &DisplayInfo,
        loaded: &FileList,
        rect_min: Pos2,
    ) -> Resolution {
        let mut order_mapping = HashMap::new();
        let mut counter = 0usize;
        for (file_index, file) in loaded.files.iter().enumerate() {
            for (order_index, order) in file.order_list.orders.iter().enumerate() {
                let pos = if self.group_by_dest {
                    let Some(dest) = order.typed.destination_id() else { continue };
                    let Some(pos) = self.dest_permutation.find_by_dest(dest) else { continue };
                    if let Some(override_) = self.override_dest_pos_once
                        && override_.0 == dest
                    {
                        override_.1
                    } else {
                        pos as f32
                    }
                } else {
                    let (OrderTyped::GoToStation { .. } | OrderTyped::GoToWaypoint { .. }) =
                        order.typed
                    else {
                        continue;
                    };
                    let pos = counter as f32;
                    counter += 1;
                    pos
                };
                order_mapping.insert(OrderRef { file_index, order_index }, pos);
            }
        }

        let px_per_tick = (self.orientation.time(avbl_size).ceil() * self.time_scale)
            / ((display.end_time - display.start_time).0 as f32);
        let num_orders = if self.group_by_dest { self.dest_permutation.len() } else { counter };
        let px_per_order =
            self.orientation.order(avbl_size) / (num_orders as f32) * self.order_scale;

        Resolution {
            px_per_tick,
            px_per_order,
            time_range: [display.start_time, display.end_time],
            num_orders,
            rect: Rect::from_min_size(rect_min, avbl_size),
            order_mapping,
        }
    }
}

#[derive(Debug)]
struct Resolution {
    px_per_tick:   f32,
    px_per_order:  f32,
    time_range:    [Ticks; 2],
    num_orders:    usize,
    rect:          Rect,
    order_mapping: HashMap<OrderRef, f32>,
}

impl Resolution {
    fn tick_to_pos(&self, config: &PlotConfig, ticks: Ticks) -> f32 {
        let tick_px =
            ((ticks - self.time_range[0] - config.time_offset).0 as f32) * self.px_per_tick;
        config.orientation.time(self.rect.min.to_vec2()) + tick_px
    }

    fn order_to_pos(&self, config: &PlotConfig, order: ResolveOrder) -> f32 {
        let resolve_order = |poo: DestOrOrder| match poo {
            DestOrOrder::Dest(dest) => {
                config.dest_permutation.find_by_dest(dest).map_or(0.0, |p| p as f32)
            }
            DestOrOrder::Order(order) => self.order_mapping.get(&order).copied().unwrap_or(0.0),
        };

        let cycle = self.num_orders as f32;
        let offset = match order {
            ResolveOrder::Exact(order) => {
                (resolve_order(order.into()) + 0.5 - config.order_offset).rem_euclid(cycle)
            }
            ResolveOrder::Lerp(prev, next, lerp) => {
                let prev = resolve_order(prev.into()) + 0.5;
                let next = resolve_order(next.into()) + 0.5;
                let change = next - prev;
                let change = if change > cycle * ORDER_WRAP_THRESHOLD {
                    change - cycle
                } else if change < -cycle * ORDER_WRAP_THRESHOLD {
                    change + cycle
                } else {
                    change
                };
                (prev + lerp * change - config.order_offset).rem_euclid(cycle)
            }
            ResolveOrder::StartBound(order) => {
                (resolve_order(order) - config.order_offset).rem_euclid(cycle)
            }
            ResolveOrder::EndBound(order) => {
                (resolve_order(order) - config.order_offset).rem_euclid(cycle) + 1.0
            }
        };

        config.orientation.order(self.rect.min.to_vec2()) + offset * self.px_per_order
    }

    /// `mapped` is a vlaue from `order_mapping`
    fn mapped_order_to_pos(&self, config: &PlotConfig, mapped: f32) -> f32 {
        let cycle = self.num_orders as f32;
        let offset = (mapped + 0.5 - config.order_offset).rem_euclid(cycle);
        config.orientation.order(self.rect.min.to_vec2()) + offset * self.px_per_order
    }

    fn tick_order_to_pos(&self, config: &PlotConfig, tick: Ticks, order: ResolveOrder) -> Pos2 {
        match config.orientation {
            Orientation::HorizontalTime => {
                pos2(self.tick_to_pos(config, tick), self.order_to_pos(config, order))
            }
            Orientation::VerticalTime => {
                pos2(self.order_to_pos(config, order), self.tick_to_pos(config, tick))
            }
        }
    }

    fn pos_to_tick_order(&self, config: &PlotConfig, pos: Pos2) -> PosToTickOrder {
        let vec = pos - self.rect.min;
        let tick_px = config.orientation.time(vec);
        let order_px = config.orientation.order(vec);
        let ticks =
            Ticks((tick_px / self.px_per_tick) as i32) + self.time_range[0] + config.time_offset;
        let order_offset = order_px / self.px_per_order + config.order_offset;
        let order_offset = order_offset.rem_euclid(self.num_orders as f32) - 0.5;
        let order = self
            .order_mapping
            .iter()
            .min_by_key(|&(_, &mapped)| OrderedFloat((order_offset - mapped).abs()))
            .map_or(OrderRef { file_index: 0, order_index: 0 }, |(&order, _)| order);
        PosToTickOrder { ticks, order, display_pos: order_offset }
    }

    fn full_tick_line(&self, config: &PlotConfig, ticks: Ticks) -> [Pos2; 2] {
        let v = self.tick_to_pos(config, ticks);
        match config.orientation {
            Orientation::HorizontalTime => [pos2(v, self.rect.min.y), pos2(v, self.rect.max.y)],
            Orientation::VerticalTime => [pos2(self.rect.min.x, v), pos2(self.rect.max.x, v)],
        }
    }

    fn full_order_line(&self, config: &PlotConfig, order: ResolveOrder) -> [Pos2; 2] {
        let v = self.order_to_pos(config, order);
        match config.orientation {
            Orientation::HorizontalTime => [pos2(self.rect.min.x, v), pos2(self.rect.max.x, v)],
            Orientation::VerticalTime => [pos2(v, self.rect.min.y), pos2(v, self.rect.max.y)],
        }
    }

    fn rendered_time_range(&self, config: &PlotConfig) -> [Ticks; 2] {
        let start_ticks = self.pos_to_tick_order(config, self.rect.min).ticks;
        let end_ticks = self.pos_to_tick_order(config, self.rect.max).ticks;
        [start_ticks, end_ticks]
    }
}

#[derive(Debug, Clone, Copy)]
struct PosToTickOrder {
    pub ticks:       Ticks,
    pub order:       OrderRef,
    pub display_pos: f32,
}

enum ResolveOrder {
    Exact(OrderRef),
    Lerp(OrderRef, OrderRef, f32),
    StartBound(DestOrOrder),
    EndBound(DestOrOrder),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VehicleColorScheme {
    OrderList,
    StartingSchedule,
    Vehicle,
}

impl fmt::Display for VehicleColorScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VehicleColorScheme::OrderList => write!(f, "Color by order List"),
            VehicleColorScheme::StartingSchedule => write!(f, "Color by starting schedule"),
            VehicleColorScheme::Vehicle => write!(f, "Color by vehicle"),
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
    order:   Option<OrderRef>,
    time:    Option<Ticks>,
    vehicle: Option<HoveredVehicle>,
}

#[derive(Debug, Clone, Copy)]
struct HoveredVehicle {
    vehicle:    VehicleRef,
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

fn draw_hovered_crosshair(
    painter: &Painter,
    plot_config: &PlotConfig,
    resolution: &Resolution,
    hovered: &Hovered,
) {
    if let Some(ticks) = hovered.time {
        painter.line_segment(
            resolution.full_tick_line(plot_config, ticks),
            Stroke::new(1.0, Color32::from_rgb(220, 130, 200)),
        );
    }

    if let Some(order) = hovered.order {
        painter.line_segment(
            resolution.full_order_line(plot_config, ResolveOrder::Exact(order)),
            Stroke::new(1.0, Color32::from_rgb(220, 160, 160)),
        );
    }
}

fn draw_time_ticks(
    painter: &Painter,
    plot_config: &PlotConfig,
    resolution: &Resolution,
    display: &DisplayInfo,
) {
    let rendered_range = resolution.rendered_time_range(plot_config);
    let interval = (rendered_range[1] - rendered_range[0]) / plot_config.time_ticks_density;
    let start = rendered_range[0] + interval / 2;

    for index in 0..plot_config.time_ticks_density {
        let ticks = start + interval * index;
        let line = resolution.full_tick_line(plot_config, ticks);
        painter.line_segment(line, Stroke::new(1.0, Color32::from_rgb(160, 80, 200)));

        let text_pos = match plot_config.orientation {
            Orientation::HorizontalTime => pos2(line[0].x, resolution.rect.min.y + 10.0),
            Orientation::VerticalTime => pos2(resolution.rect.min.x + 10.0, line[0].y),
        };
        if let Ok(timestamp) = Timestamp::from_ticks(ticks, display.tpm) {
            painter.text(
                text_pos,
                Align2::CENTER_TOP,
                timestamp.to_string(),
                FontId::new(12.0, FontFamily::Proportional),
                Color32::WHITE,
            );
        }
    }
}

fn detect_hover(
    resp: &egui::Response,
    hovered: &mut HoveredPair,
    plot_config: &mut PlotConfig,
    resolution: &Resolution,
    persist: &mut Persist,
) {
    let tick_order =
        resp.hover_pos().map(|hover| resolution.pos_to_tick_order(&*plot_config, hover));
    if let Some(hover) = resp.hover_pos()
        && let Some(tick_order) = tick_order
    {
        hovered.next.time = Some(tick_order.ticks);
        hovered.next.order = Some(tick_order.order);

        let display_pos = tick_order.display_pos.round() as usize;
        if plot_config.group_by_dest
            && let Some(dest) = plot_config.dest_permutation.get_at_pos(display_pos)
        {
            resp.dnd_set_drag_payload(DndPayload {
                dest,
                initial_pos: hover,
                initial_order: display_pos,
            });
        }
    }

    if let Some(dnd) = resp.dnd_hover_payload::<DndPayload>() {
        plot_config.override_dest_pos_once = resp.hover_pos().map(|hover| {
            let moved_px = plot_config.orientation.order(hover - dnd.initial_pos);
            (dnd.dest, dnd.initial_order as f32 + moved_px / resolution.px_per_order)
        });
    }
    if let Some(dnd) = resp.dnd_release_payload::<DndPayload>() {
        plot_config.override_dest_pos_once = None;
        if let Some(tick_order) = tick_order {
            plot_config.dest_permutation.relocate(
                dnd.initial_order,
                tick_order.display_pos.round() as usize,
                persist,
            );
        }
    }
}

struct DndPayload {
    dest:          DestinationId,
    initial_pos:   Pos2,
    initial_order: usize,
}

fn show_tooltip(
    resp: &egui::Response,
    hovered: &Hovered,
    loaded: &FileList,
    display: &DisplayInfo,
    config: &PlotConfig,
    resolution: &Resolution,
    force_show: bool,
    additional: impl FnOnce(&mut egui::Ui),
) {
    if !force_show && hovered.time.is_none() && hovered.order.is_none() {
        return;
    }

    let pos = if let Some(time) = hovered.time
        && let Some(order) = hovered.order
    {
        resolution.tick_order_to_pos(config, time, ResolveOrder::Exact(order))
    } else {
        resolution.rect.center()
    };

    Popup::from_response(resp)
        .kind(PopupKind::Tooltip)
        .sense(egui::Sense::empty())
        .at_position(pos)
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
                && let Some(order) =
                    loaded.files[order.file_index].order_list.orders.get(order.order_index)
                && let Some(name) = order.typed.destination_name()
            {
                ui.label(format!("Order: {name}"));
            }

            additional(ui);
        });
}

#[derive(Debug, Default)]
struct DestPermutation {
    sorted: IndexSet<DestinationId>,
}

impl DestPermutation {
    fn len(&self) -> usize { self.sorted.len() }

    fn resize(&mut self, files: &FileList, persist: &mut Persist) {
        let required: IndexSet<_> = files
            .files
            .iter()
            .flat_map(|file| &file.order_list.orders)
            .filter_map(|order| order.typed.destination_id())
            .collect();
        self.sorted.retain(|v| required.contains(v));

        for dest in required {
            if self.sorted.contains(&dest) {
                continue;
            }

            if persist.data.dest_order.insert(dest) {
                persist.changed = true;
            }
            self.sorted.insert_sorted_by_key(dest, |d| {
                persist.data.dest_order.get_index_of(d).unwrap_or(persist.data.dest_order.len())
            });
        }
    }

    fn find_by_dest(&self, dest: DestinationId) -> Option<usize> { self.sorted.get_index_of(&dest) }

    fn get_at_pos(&self, pos: usize) -> Option<DestinationId> {
        self.sorted.get_index(pos).copied()
    }

    fn iter(&self) -> impl Iterator<Item = (usize, DestinationId)> + '_ {
        self.sorted.iter().copied().enumerate()
    }

    fn relocate(&mut self, from: usize, to: usize, persist: &mut Persist) {
        if from == to || from >= self.sorted.len() {
            return;
        }
        let to = to.min(self.sorted.len() - 1);
        self.sorted.move_index(from, to);
        let subj = self.sorted.get_index(to).unwrap();

        let persist_order = &mut persist.data.dest_order;
        if let Some(prev) = to.checked_sub(1) {
            // use to-1 as anchor for persist relocation
            let prev_dest = self.sorted.get_index(prev).expect("prev < to < sorted.len()");
            if let Some(prev_persist_index) = persist_order.get_index_of(prev_dest)
                && let Some(subj_persist_index) = persist_order.get_index_of(subj)
            {
                let move_target = if prev_persist_index < subj_persist_index {
                    // prev_persist_index is unchanged after moving subj
                    prev_persist_index + 1
                } else {
                    // prev_persist_index would shift leftward by one,
                    // so subj would take its original place.
                    prev_persist_index
                };
                persist_order
                    .move_index(subj_persist_index, move_target.min(persist_order.len() - 1));
            }
        } else if let Some(next_dest) = self.sorted.get_index(to + 1) {
            // use to+1 as anchor for persist relocation
            if let Some(next_persist_index) = persist_order.get_index_of(next_dest)
                && let Some(subj_persist_index) = persist_order.get_index_of(subj)
            {
                let move_target = if next_persist_index > subj_persist_index {
                    // next_persist_index is unchanged after moving subj
                    next_persist_index - 1 // 0 <= subj_persist_index < next_persist_index
                } else {
                    // next_persist_index would shift rightward by one,
                    // so subj would take its original place.
                    next_persist_index
                };
                persist_order.move_index(subj_persist_index, move_target);
            }
        }
        persist.changed = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct OrderRef {
    file_index:  usize,
    order_index: usize,
}

impl OrderRef {
    fn get(self, loaded: &FileList) -> Option<&schema::Order> {
        loaded
            .files
            .get(self.file_index)
            .and_then(|file| file.order_list.orders.get(self.order_index))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum DestOrOrder {
    Dest(DestinationId),
    Order(OrderRef),
}

impl From<OrderRef> for DestOrOrder {
    fn from(value: OrderRef) -> Self { Self::Order(value) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct VehicleRef {
    file_index:    usize,
    vehicle_index: usize,
}

fn vehicle_stroke_color(
    vehicle: VehicleRef,
    plot_config: &PlotConfig,
    vehicle_list: &VehicleList,
    palette: &mut Palette,
) -> Color32 {
    match plot_config.vehicle_color_scheme {
        VehicleColorScheme::OrderList => hash_to_rgb(palette, vehicle.file_index),
        VehicleColorScheme::StartingSchedule => hash_to_rgb(
            palette,
            (
                vehicle.file_index,
                vehicle_list.vehicles[vehicle.vehicle_index].starting_schedule_index,
            ),
        ),
        VehicleColorScheme::Vehicle => hash_to_rgb(palette, vehicle),
    }
}

#[derive(Default)]
struct Palette {
    colors: HashMap<u64, Color32>,
}

fn hash_to_rgb(palette: &mut Palette, salt: impl fmt::Debug + Hash + Any) -> Color32 {
    let mut hasher = DefaultHasher::new();
    salt.type_id().hash(&mut hasher);
    salt.hash(&mut hasher);
    let hash = hasher.finish();

    *palette.colors.entry(hash).or_insert_with(|| {
        let hash_u16 =
            (hash as u16) ^ ((hash >> 16) as u16) ^ ((hash >> 32) as u16) ^ ((hash >> 48) as u16);
        let hue = GOLDEN_RATIO.fract() * (hash_u16 as f32 / u16::MAX as f32);
        Color32::from(Hsva::new(hue.fract(), 0.65, 0.95, 1.0))
    })
}
