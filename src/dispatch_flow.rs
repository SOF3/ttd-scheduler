use std::cmp;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context as _, Result};

use crate::schema::{
    Condition, Duration, JumpToken, OrderList, OrderTyped, Schedule, SlotSource, Ticks, Timestamp,
    TrueOrFalse, read_order_list,
};

#[derive(clap::Args)]
pub struct Command {
    /// Path to orderlist file
    pub order_list: PathBuf,

    #[clap(flatten)]
    pub args: Args,
}

#[derive(clap::Args, Clone, PartialEq, Eq)]
pub struct Args {
    /// Use this as the starting time.
    #[clap(long, value_name = "HH:MM", value_parser)]
    pub start_time:      Option<Timestamp>,
    /// Initial number of vehicles at each schedule assignment.
    #[clap(
        long,
        value_name = "<SCHEDULE_INDEX>:<COUNT>,<SCHEDULE_INDEX>:<COUNT>,...",
        value_parser,
        default_value = ""
    )]
    pub initial:         InitialVehicles,
    /// Duration to simulate.
    #[clap(long, default_value = "24:00", value_parser)]
    pub simulation_time: Duration,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            start_time:      None,
            initial:         InitialVehicles::default(),
            simulation_time: Duration { hours: 24, minutes: 0, seconds: 0 },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitialVehicles(pub Vec<u32>);

impl FromStr for InitialVehicles {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut counts = Vec::new();
        for item in s.split(',').filter(|s| !s.is_empty()) {
            let (index, count) = item
                .split_once(':')
                .context("initial vehicles must be in the format <SCHEDULE_INDEX>:<COUNT>")?;
            let index: usize = index.parse().context("parse schedule index")?;
            let count: u32 = count.parse().context("parse vehicle count")?;
            counts.resize(index + 1, 0);
            counts[index] = count;
        }
        Ok(Self(counts))
    }
}

impl Command {
    pub fn run(&self) -> Result<()> {
        let mut logger = StatsLogger::default();
        let order_list = read_order_list(&self.order_list)?;
        let display = self.args.simulate(&order_list, &mut logger)?;

        for snapshot in logger.snapshots {
            let mut orders = String::new();
            for order in snapshot.orders {
                use std::fmt::Write;
                let appr: u32 = order.approaching.values().sum();
                let wait = order.waiting;
                write!(orders, " {appr:2} {wait:2}").unwrap();
            }

            println!("{}{orders}", Timestamp::from_ticks(snapshot.time, display.tpm)?);
        }

        Ok(())
    }
}

impl Args {
    pub fn simulate(
        &self,
        order_list: &OrderList,
        logger: &mut impl Logger,
    ) -> Result<DisplayInfo> {
        let tpm = order_list.game_properties.ticks_per_minute;
        let day_length = tpm * 1440;

        for (i, schedule) in order_list.schedules.iter().enumerate() {
            anyhow::ensure!(
                schedule.duration == day_length,
                "schedule {} is not a 24-hour schedule",
                i + 1
            );
        }

        let start_time = if let Some(start_time) = self.start_time {
            start_time.to_ticks(tpm)
        } else {
            let Some(first_schedule) = order_list.schedules.first() else {
                anyhow::bail!("no schedules in order list");
            };
            let first_start = first_schedule.relative_start_time;
            if order_list
                .schedules
                .iter()
                .any(|schedule| schedule.relative_start_time != first_start)
            {
                anyhow::bail!("schedules have different start times");
            }
            first_start
        };

        let ctx = Context {
            order_list,
            jump_index: order_list
                .orders
                .iter()
                .enumerate()
                .filter_map(|(i, order)| order.jump_from.clone().map(|token| (token, i)))
                .collect(),
            start_time,
            day_length,
            tpm,
        };
        let mut state = State {
            scheduler_heap: BinaryHeap::new(),
            schedules:      order_list
                .schedules
                .iter()
                .enumerate()
                .map(|(index, schedule)| {
                    Ok(ScheduleState {
                        assign_order:         order_list
                            .orders
                            .iter()
                            .position(|order| order.schedule_index == Some(index))
                            .with_context(|| {
                                format!("schedule {} not assigned to any order", index + 1)
                            })?,
                        next_dispatch_slot:   schedule
                            .slots
                            .iter()
                            .enumerate()
                            .map(|(slot, data)| {
                                (
                                    slot,
                                    data.timestamp(schedule).mod_start_time(start_time, day_length),
                                )
                            })
                            .min_by_key(|&(_, timestamp)| timestamp)
                            .map(|(slot, _)| slot)
                            .unwrap_or(0),
                        dispatch_slot_cycles: Ticks(0),
                        last_dispatch_time:   None,
                    })
                })
                .collect::<Result<_>>()?,
            vehicles:       Vec::new(),
        };

        state.apply_initial(&ctx, logger, &self.initial).context("apply initial vehicles")?;

        logger.step(order_list, start_time, &state);

        while let Some((cmp::Reverse(time), vehicle_id)) = state.scheduler_heap.pop() {
            state.spin_once(&ctx, logger, time, vehicle_id)?;
            if state
                .scheduler_heap
                .peek()
                .is_none_or(|&(cmp::Reverse(next_time), _)| next_time != time)
            {
                logger.step(order_list, time, &state);
            }
            if time > ctx.start_time + self.simulation_time.to_ticks(tpm) {
                break;
            }
        }

        Ok(DisplayInfo {
            tpm,
            start_time,
            end_time: start_time + self.simulation_time.to_ticks(tpm),
        })
    }
}

#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub tpm:        Ticks,
    pub start_time: Ticks,
    pub end_time:   Ticks,
}

pub trait Logger {
    fn step(&mut self, order_list: &OrderList, time: Ticks, state: &State);

    fn missed_dispatch_slot(&mut self, schedule_index: usize, slot: usize, time: Ticks);
}

struct Context<'a> {
    order_list: &'a OrderList,
    jump_index: HashMap<JumpToken, usize>,
    start_time: Ticks,
    day_length: Ticks,
    tpm:        Ticks,
}

pub struct State {
    scheduler_heap: BinaryHeap<(cmp::Reverse<Ticks>, usize)>,
    pub vehicles:   Vec<Vehicle>,
    pub schedules:  Vec<ScheduleState>,
}

impl State {
    fn apply_initial(
        &mut self,
        ctx: &Context,
        logger: &mut impl Logger,
        initial: &InitialVehicles,
    ) -> Result<()> {
        for (((schedule_index, schedule), schedule_state), &num_vehicles) in
            ctx.order_list.schedules.iter().enumerate().zip(&mut self.schedules).zip(&initial.0)
        {
            for _ in 0..num_vehicles {
                let (dispatch_slot, dispatch_time) = schedule_state.next_dispatch_slot(
                    logger,
                    schedule_index,
                    schedule,
                    ctx.start_time,
                )?;

                let vehicle_id = self.vehicles.len();
                let mut schedule_slots = vec![None; schedule.slots.len()];
                schedule_slots[schedule_index] = Some(dispatch_slot);
                self.vehicles.push(Vehicle {
                    current_order: schedule_state.assign_order,
                    last_order: None,
                    action: VehicleAction::PendingDispatch,
                    schedule_slots,
                });
                self.scheduler_heap.push((cmp::Reverse(dispatch_time), vehicle_id));
            }
        }
        Ok(())
    }

    fn spin_once(
        &mut self,
        ctx: &Context,
        logger: &mut impl Logger,
        current_time: Ticks,
        vehicle_id: usize,
    ) -> Result<()> {
        let vehicle = &mut self.vehicles[vehicle_id];
        let eta = match vehicle.action {
            VehicleAction::Waiting | VehicleAction::PendingDispatch => {
                // departure
                vehicle.next_order(ctx, current_time)?
            }
            VehicleAction::Traveling { .. } => {
                // arrival
                let order = &ctx.order_list.orders[vehicle.current_order];
                let (eta, action) = if let Some(schedule_index) = order.schedule_index {
                    let schedule = ctx
                        .order_list
                        .schedules
                        .get(schedule_index)
                        .context("reference to invalid schedule")?;
                    let (slot_index, eta) = self.schedules[schedule_index]
                        .next_dispatch_slot(logger, schedule_index, schedule, current_time)
                        .context("acquire next dispatch slot")?;
                    vehicle.schedule_slots[schedule_index] = Some(slot_index);

                    (eta, VehicleAction::PendingDispatch)
                } else {
                    match order.typed {
                        OrderTyped::GoToStation { wait_time, .. } => {
                            (current_time + wait_time, VehicleAction::Waiting)
                        }
                        OrderTyped::GoToWaypoint { .. } => (current_time, VehicleAction::Waiting),
                        OrderTyped::Conditional { .. } => unreachable!(
                            "vehicle must not be put on heap when current order is conditional"
                        ),
                    }
                };
                vehicle.action = action;
                eta
            }
        };
        self.scheduler_heap.push((cmp::Reverse(eta), vehicle_id));
        Ok(())
    }
}

pub struct ScheduleState {
    pub assign_order:         usize,
    // slot index is 0 if schedule is empty.
    pub next_dispatch_slot:   usize,
    pub dispatch_slot_cycles: Ticks,
    pub last_dispatch_time:   Option<Ticks>,
}

impl ScheduleState {
    fn next_dispatch_slot(
        &mut self,
        logger: &mut impl Logger,
        schedule_index: usize,
        schedule: &Schedule,
        current_time: Ticks,
    ) -> Result<(usize, Ticks)> {
        let num_slots = schedule.slots.len();
        anyhow::ensure!(num_slots > 0, "schedule has no slots and cannot dispatch vehicles");
        loop {
            let slot = self.next_dispatch_slot;
            self.next_dispatch_slot += 1;
            if self.next_dispatch_slot >= num_slots {
                self.next_dispatch_slot = 0;
            }
            let slot_data = &schedule.slots[self.next_dispatch_slot];
            let mut dispatch_time = slot_data.timestamp(schedule) + self.dispatch_slot_cycles;
            if let Some(last) = self.last_dispatch_time
                && dispatch_time <= last
            {
                self.dispatch_slot_cycles += schedule.duration;
                dispatch_time += schedule.duration;
            }
            self.last_dispatch_time = Some(dispatch_time);
            if dispatch_time + schedule.max_delay >= current_time {
                return Ok((slot, dispatch_time.max(current_time)));
            }
            logger.missed_dispatch_slot(schedule_index, slot, dispatch_time);
        }
    }
}

pub struct Vehicle {
    pub current_order:  usize,
    /// If last order is present, the order index and departure time.
    pub last_order:     Option<(usize, Ticks)>,
    pub action:         VehicleAction,
    pub schedule_slots: Vec<Option<usize>>,
}

pub enum VehicleAction {
    /// Pending dispatch at the current slot.
    PendingDispatch,
    /// Traveling to the destination of `current_order`.
    ///
    /// `eta` is the time of arrival, excluding waiting time.
    ///
    /// This requires `last_order` to be `Some`.
    Traveling { eta: Ticks },
    /// Waiting at the destination of `current_order`.
    ///
    /// Will move to the next order at `departure`.
    Waiting,
}

impl Vehicle {
    /// Returns the ETA.
    fn next_order(&mut self, ctx: &Context, current_time: Ticks) -> Result<Ticks> {
        self.last_order = Some((self.current_order, current_time));
        self.current_order += 1;
        self.current_order %= ctx.order_list.orders.len();

        let mut override_travel_time = None;

        loop {
            let order = &ctx.order_list.orders[self.current_order];
            match order.typed {
                OrderTyped::GoToStation { travel_time, .. }
                | OrderTyped::GoToWaypoint { travel_time, .. } => {
                    let travel_time = override_travel_time.unwrap_or(travel_time);
                    let eta = current_time + travel_time;
                    self.action = VehicleAction::Traveling { eta };
                    break Ok(eta);
                }
                OrderTyped::Conditional { ref jump_to, jump_taken_travel_time, ref condition } => {
                    let matched = match *condition {
                        Condition::Always => true,
                        Condition::DispatchSlot {
                            ref condition_slot_source,
                            condition_dispatch_schedule,
                            condition_check_tag,
                            condition_check_route,
                            condition_comparator,
                        } => {
                            let matched = match *condition_slot_source {
                                SlotSource::Vehicle => {
                                    let mut matched = false;
                                    let schedule_slot = self
                                        .schedule_slots
                                        .get(condition_dispatch_schedule)
                                        .copied()
                                        .flatten();
                                    if let Some(slot) = schedule_slot
                                        && let Some(slot) = ctx.order_list.schedules
                                            [condition_dispatch_schedule]
                                            .slots
                                            .get(slot)
                                    {
                                        if let Some(tag) = condition_check_tag
                                            && slot.has_tag(tag)
                                        {
                                            matched = true;
                                        }
                                        if let Some(route) = condition_check_route
                                            && slot.has_route(route)
                                        {
                                            matched = true;
                                        }
                                        // TODO other schdule condition
                                    }
                                    matched
                                }
                            };
                            matched == (condition_comparator == TrueOrFalse::True)
                        }
                    };
                    if matched {
                        self.current_order =
                            *ctx.jump_index.get(jump_to).context("invalid jump target")?;
                        override_travel_time.get_or_insert(jump_taken_travel_time);
                    } else {
                        self.current_order += 1;
                        self.current_order %= ctx.order_list.orders.len();
                    }
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct StatsLogger {
    pub snapshots: Vec<StatsLoggerSnapshot>,
}

#[derive(Debug)]
pub struct StatsLoggerSnapshot {
    pub time:   Ticks,
    pub orders: Vec<StatsLoggerSnapshotOrder>,
}

#[derive(Debug, Default)]
pub struct StatsLoggerSnapshotOrder {
    pub waiting:     u32,
    pub approaching: HashMap<usize, u32>,
}

impl Logger for StatsLogger {
    fn step(&mut self, order_list: &OrderList, time: Ticks, state: &State) {
        let mut snapshot = StatsLoggerSnapshot { time, orders: Vec::new() };
        snapshot.orders.resize_with(order_list.orders.len(), Default::default);
        for vehicle in &state.vehicles {
            match vehicle.action {
                VehicleAction::PendingDispatch | VehicleAction::Waiting => {
                    snapshot.orders[vehicle.current_order].waiting += 1;
                }
                VehicleAction::Traveling { .. } => {
                    let appr = &mut snapshot.orders[vehicle.current_order].approaching;
                    let count = appr
                        .entry(vehicle.last_order.expect("traveling must have last order").0)
                        .or_default();
                    *count += 1;
                }
            }
        }
        self.snapshots.push(snapshot);
    }

    fn missed_dispatch_slot(&mut self, schedule_index: usize, slot: usize, time: Ticks) {
        println!("Missed dispatch slot: schedule {schedule_index}, slot {slot}, time {time:?}");
    }
}
