use std::collections::HashMap;
use std::fmt::{self, Display};
use std::path::Path;
use std::str::FromStr;
use std::{fs, ops};

use anyhow::Context;
use arrayvec::ArrayVec;
use derive_more::{Add, AddAssign, Sub, SubAssign};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub struct OrderList {
    pub orders:          Vec<Order>,
    pub schedules:       Vec<Schedule>,
    pub game_properties: GameProperties,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub struct GameProperties {
    pub ticks_per_minute: Ticks,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub struct Order {
    pub jump_from:      Option<JumpToken>,
    #[serde(flatten)]
    pub typed:          OrderTyped,
    pub schedule_index: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum OrderTyped {
    #[serde(rename_all = "kebab-case")]
    GoToStation {
        destination_id:       u32,
        destination_name:     String,
        destination_location: Coords,
        travel_time:          Ticks,
        wait_time:            Ticks,
    },
    #[serde(rename_all = "kebab-case")]
    GoToWaypoint {
        destination_id:       u32,
        destination_name:     String,
        destination_location: Coords,
        travel_time:          Ticks,
    },
    #[serde(rename_all = "kebab-case")]
    Conditional {
        jump_to:                JumpToken,
        jump_taken_travel_time: Ticks,
        #[serde(flatten)]
        condition:              Condition,
    },
}

impl OrderTyped {
    pub fn destination_name(&self) -> Option<&str> {
        match self {
            OrderTyped::GoToStation { destination_name, .. }
            | OrderTyped::GoToWaypoint { destination_name, .. } => Some(destination_name),
            OrderTyped::Conditional { .. } => None,
        }
    }

    pub fn destination_location(&self) -> Option<Coords> {
        match *self {
            OrderTyped::GoToStation { destination_location, .. }
            | OrderTyped::GoToWaypoint { destination_location, .. } => Some(destination_location),
            OrderTyped::Conditional { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Coords {
    #[serde(rename = "X")]
    pub x: u32,
    #[serde(rename = "Y")]
    pub y: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Schedule {
    pub duration:            Ticks,
    #[serde(default)]
    pub max_delay:           Ticks,
    pub relative_start_time: Ticks,
    #[serde(default)]
    pub route_ids:           HashMap<String, String>,
    pub slots:               Vec<ScheduleSlot>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleSlot {
    OffsetOnly(Ticks),
    Composite {
        offset: Ticks,
        #[serde(default)]
        tags:   ArrayVec<Tag, 4>,
        route:  Option<Route>,
    },
}

impl ScheduleSlot {
    pub fn timestamp(&self, schedule: &Schedule) -> Ticks {
        let offset = match *self {
            ScheduleSlot::OffsetOnly(offset) | ScheduleSlot::Composite { offset, .. } => offset,
        };
        schedule.relative_start_time + offset
    }

    pub fn has_tag(&self, tag: Tag) -> bool {
        match self {
            ScheduleSlot::OffsetOnly(_) => false,
            ScheduleSlot::Composite { tags, .. } => tags.contains(&tag),
        }
    }

    pub fn has_route(&self, route: Route) -> bool {
        match *self {
            ScheduleSlot::OffsetOnly(_) => false,
            ScheduleSlot::Composite { route: slot_route, .. } => slot_route == Some(route),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    Add,
    AddAssign,
    Sub,
    SubAssign,
)]
pub struct Ticks(pub i32);

impl ops::Mul<usize> for Ticks {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self::Output { Ticks(self.0 * i32::try_from(rhs).unwrap()) }
}

impl ops::Div<usize> for Ticks {
    type Output = Self;

    fn div(self, rhs: usize) -> Self::Output { Ticks(self.0 / i32::try_from(rhs).unwrap()) }
}

impl Ticks {
    /// Used on [`Ticks`] that represent timestamps,
    /// wraps to the range `start_time..start_time+day_length`.
    #[must_use]
    pub fn mod_start_time(mut self, start_time: Ticks, day_length: Ticks) -> Ticks {
        self.0 %= day_length.0;
        if self.0 < start_time.0 {
            self.0 += day_length.0;
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JumpToken(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tag(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Route(pub u8);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "condition-variable")]
pub enum Condition {
    Always,
    #[serde(rename_all = "kebab-case")]
    DispatchSlot {
        condition_slot_source:       SlotSource,
        condition_dispatch_schedule: usize,
        condition_check_tag:         Option<Tag>,
        condition_check_route:       Option<Route>,
        condition_comparator:        TrueOrFalse,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrueOrFalse {
    False,
    True,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlotSource {
    Vehicle,
}

pub fn read_order_list(path: &Path) -> anyhow::Result<OrderList> {
    try_read_order_list(path).with_context(|| format!("read order list at {}", path.display()))
}
fn try_read_order_list(path: &Path) -> anyhow::Result<OrderList> {
    let contents = fs::read_to_string(path).context("open config")?;
    serde_json::from_str(&contents).context("parse config")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub hours:   u8,
    pub minutes: u8,
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hours, self.minutes)
    }
}

impl FromStr for Timestamp {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s.split_once(':').context("timestamp must be in the format HH:MM")?;
        let hours = parts.0.parse::<u8>().context("parse hours")?;
        let minutes = parts.1.parse::<u8>().context("parse minutes")?;
        Ok(Timestamp { hours, minutes })
    }
}

impl Timestamp {
    pub fn from_ticks(ticks: Ticks, ticks_per_minute: Ticks) -> anyhow::Result<Self> {
        let minutes = ticks.0 / ticks_per_minute.0;
        let hours = minutes / 60;
        let minutes = minutes % 60;
        Ok(Self {
            hours:   hours.try_into().context("hours out of range")?,
            minutes: minutes.try_into().context("ticks must be positive")?,
        })
    }

    pub fn to_ticks(self, ticks_per_minute: Ticks) -> Ticks {
        let minutes = usize::from(self.hours) * 60 + usize::from(self.minutes);
        ticks_per_minute * minutes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration {
    pub hours:   u8,
    pub minutes: u8,
    pub seconds: u8,
}

impl Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:02}:{:02}", self.hours, self.minutes, self.seconds)
    }
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (h, m, s) = match s.split_once(':') {
            None => ("0", s, "0"),
            Some((hours, rest)) => match rest.split_once(':') {
                None => (hours, rest, "0"),
                Some((minutes, seconds)) => (hours, minutes, seconds),
            },
        };
        Ok(Self {
            hours:   h.parse::<u8>().context("parse hours")?,
            minutes: m.parse::<u8>().context("parse minutes")?,
            seconds: s.parse::<u8>().context("parse seconds")?,
        })
    }
}

impl Duration {
    pub fn from_ticks(ticks: Ticks, ticks_per_minute: Ticks) -> anyhow::Result<Self> {
        let mut total = (ticks.0 * 60) / ticks_per_minute.0;
        let seconds = total % 60;
        total /= 60;
        let minutes = total % 60;
        total /= 60;
        let hours = total;
        Ok(Self {
            hours:   hours.try_into().context("hours out of range")?,
            minutes: minutes.try_into().context("minutes out of range")?,
            seconds: seconds.try_into().context("seconds out of range")?,
        })
    }

    pub fn to_ticks(self, ticks_per_minute: Ticks) -> Ticks {
        let num_seconds = usize::from(self.hours) * 3600
            + usize::from(self.minutes) * 60
            + usize::from(self.seconds);
        ticks_per_minute * num_seconds / 60
    }
}
