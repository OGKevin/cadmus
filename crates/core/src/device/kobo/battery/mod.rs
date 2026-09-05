//! Kobo battery via Linux power-supply sysfs.
//!
//! At construction, [`KoboBattery::new`] picks the first existing path among
//! known SoC fuel-gauge nodes under `/sys/class/power_supply/`
//! (`bd71827_bat`, `mc13892_bat`, then generic `battery`) and records the
//! `capacity` and `status` attribute paths.
//!
//! Each [`Battery`] read opens those files, reads once, and closes them so the
//! type stays [`Send`] + [`Sync`] without holding seekable file descriptors.
//! Capacity is a percent float; status strings map to [`Status`]
//! (`Discharging`, `Charging`, `Not charging` / `Full` → [`Status::Charged`]).
//!
//! When `has_power_cover` is true, an optional SleepCover / cilix auxiliary
//! pack is wired from `/sys/class/misc/cilix` (`cilix_conn`,
//! `cilix_bat_capacity`, `charge_status`). While connected (`cilix_conn == 1`),
//! [`Battery::capacity`] and [`Battery::status`] return `[main, cover]`;
//! otherwise only the main cell.

use crate::device::battery::{Battery, Status};
use anyhow::Error;
#[cfg(not(test))]
use anyhow::format_err;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(not(test))]
const BATTERY_INTERFACES: [&str; 3] = [
    "/sys/class/power_supply/bd71827_bat",
    "/sys/class/power_supply/mc13892_bat",
    "/sys/class/power_supply/battery",
];
#[cfg(not(test))]
const POWER_COVER_INTERFACE: &str = "/sys/class/misc/cilix";

#[cfg(not(test))]
const BATTERY_CAPACITY: &str = "capacity";
#[cfg(not(test))]
const BATTERY_STATUS: &str = "status";

#[cfg(not(test))]
const POWER_COVER_CAPACITY: &str = "cilix_bat_capacity";
#[cfg(not(test))]
const POWER_COVER_STATUS: &str = "charge_status";
#[cfg(not(test))]
const POWER_COVER_CONNECTED: &str = "cilix_conn";

/// Sysfs paths for the optional cilix / SleepCover battery pack.
struct PowerCover {
    capacity: PathBuf,
    status: PathBuf,
    connected: PathBuf,
}

/// Device battery backend that samples Kobo power-supply sysfs on each read.
///
/// Path discovery runs once in [`Self::new`]; subsequent reads reopen sysfs
/// files so the type is [`Send`] + [`Sync`] and safe to share via [`Arc`].
/// See the [module docs](self) for interface discovery and power-cover behavior.
// TODO: health, technology, time_to_full_now, time_to_empty_now
pub struct KoboBattery {
    capacity: PathBuf,
    status: PathBuf,
    power_cover: Option<PowerCover>,
}

impl KoboBattery {
    /// Discovers the onboard fuel-gauge sysfs node and optional cilix paths.
    ///
    /// When `has_power_cover` is true, auxiliary SleepCover sysfs nodes are
    /// recorded; capacity and status reads include a second element only while
    /// the cover reports connected (`cilix_conn == 1`).
    ///
    /// In unit tests, returns stub `/dev/null` paths so construction succeeds
    /// without hardware.
    pub fn new(has_power_cover: bool) -> Result<KoboBattery, Error> {
        cfg_select! {
            test => {
                let _ = has_power_cover;
                Ok(KoboBattery {
                    capacity: PathBuf::from("/dev/null"),
                    status: PathBuf::from("/dev/null"),
                    power_cover: None,
                })
            }
            _ => {
                let base = Path::new(
                    BATTERY_INTERFACES
                        .iter()
                        .find(|bi| Path::new(bi).exists())
                        .ok_or_else(|| format_err!("battery path missing"))?,
                );
                let capacity = base.join(BATTERY_CAPACITY);
                let status = base.join(BATTERY_STATUS);
                let power_cover = if has_power_cover {
                    let base = Path::new(POWER_COVER_INTERFACE);
                    Some(PowerCover {
                        capacity: base.join(POWER_COVER_CAPACITY),
                        status: base.join(POWER_COVER_STATUS),
                        connected: base.join(POWER_COVER_CONNECTED),
                    })
                } else {
                    None
                };
                Ok(KoboBattery {
                    capacity,
                    status,
                    power_cover,
                })
            }
        }
    }
}

fn read_sysfs(path: &Path) -> Result<String, Error> {
    Ok(fs::read_to_string(path)?)
}

impl KoboBattery {
    fn is_power_cover_connected(&self) -> Result<bool, Error> {
        let Some(power_cover) = self.power_cover.as_ref() else {
            return Ok(false);
        };
        let buf = read_sysfs(&power_cover.connected)?;
        Ok(buf.trim_end().parse::<u8>().is_ok_and(|v| v == 1))
    }
}

impl Battery for KoboBattery {
    fn capacity(&self) -> Result<Vec<f32>, Error> {
        let buf = read_sysfs(&self.capacity)?;
        let capacity = buf.trim_end().parse::<f32>().unwrap_or(0.0);
        if matches!(self.is_power_cover_connected(), Ok(true)) {
            let aux = self
                .power_cover
                .as_ref()
                .and_then(|power_cover| read_sysfs(&power_cover.capacity).ok())
                .map(|buf| buf.trim_end().parse::<f32>().unwrap_or(0.0))
                .unwrap_or(0.0);
            Ok(vec![capacity, aux])
        } else {
            Ok(vec![capacity])
        }
    }

    fn status(&self) -> Result<Vec<Status>, Error> {
        let buf = read_sysfs(&self.status)?;
        let status = match buf.trim_end() {
            "Discharging" => Status::Discharging,
            "Charging" => Status::Charging,
            "Not charging" | "Full" => Status::Charged,
            _ => Status::Unknown,
        };
        if matches!(self.is_power_cover_connected(), Ok(true)) {
            let aux_status = self
                .power_cover
                .as_ref()
                .and_then(|power_cover| read_sysfs(&power_cover.status).ok())
                .map(|buf| match buf.trim_end().parse::<i8>() {
                    Ok(0) => Status::Discharging,
                    Ok(2) => Status::Charging,
                    Ok(3) => Status::Charged,
                    _ => Status::Unknown,
                })
                .unwrap_or(Status::Unknown);
            Ok(vec![status, aux_status])
        } else {
            Ok(vec![status])
        }
    }
}
