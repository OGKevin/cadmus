//! Runtime migrations for the settings subsystem.
//!
//! # Registered migrations
//!
//! | Module | Migration ID |
//! |---|---|
//! | [`migrate_sketch_pen_speed_mm::MIGRATION_ID`] | `v1_migrate_sketch_pen_speed_mm` |

use crate::settings::PEN_MAX_SPEED_MM;
use crate::settings::Pen;
use crate::settings::versioned::SettingsManager;
use crate::unit::MILLIMETERS_PER_INCH;
use crate::version::get_current_version;

fn px_per_sec_to_mm_per_sec(px: f32, dpi: u16) -> f32 {
    px * MILLIMETERS_PER_INCH / dpi as f32
}

/// Converts legacy sketch pen speed thresholds from pixels/sec to mm/s.
///
/// Before the device-agnostic refactor, [`Pen::default`](crate::settings::Pen)
/// baked `mm_to_px(254.0, device_dpi)` into settings, so on-disk values were
/// device-specific pixels/sec. Pen speeds are now stored as mm/s and converted
/// with [`mm_to_px`](crate::unit::mm_to_px) at draw time.
///
/// Values above [`PEN_MAX_SPEED_MM`] are treated as legacy px/s because valid
/// mm/s speeds cannot exceed that maximum. Already-migrated mm/s values are
/// left unchanged so the migration is idempotent.
///
/// Returns `true` if either speed field was converted.
fn migrate_pen_speed_units_from_px(pen: &mut Pen, dpi: u16) -> bool {
    let mut changed = false;
    if pen.max_speed > PEN_MAX_SPEED_MM {
        pen.max_speed = px_per_sec_to_mm_per_sec(pen.max_speed, dpi);
        changed = true;
    }
    if pen.min_speed > PEN_MAX_SPEED_MM {
        pen.min_speed = px_per_sec_to_mm_per_sec(pen.min_speed, dpi);
        changed = true;
    }
    changed
}

crate::migration!(
    /// Converts sketch pen speed thresholds from legacy pixel/sec values
    /// (stored when defaults used device-native mm_to_px) to mm/s.
    "v1_migrate_sketch_pen_speed_mm",
    async fn migrate_sketch_pen_speed_mm(ctx: &crate::db::migrations::MigrationContext<'_>) {
        let manager = SettingsManager::new(ctx.device.data_dir.clone(), get_current_version());
        let mut settings = manager.load();
        if !migrate_pen_speed_units_from_px(&mut settings.sketch.pen, ctx.device.dpi) {
            return Ok(());
        }
        manager.save(&settings)?;
        Ok(())
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Pen;

    #[test]
    fn test_migrate_custom_legacy_max_speed_below_old_threshold() {
        let mut pen = Pen {
            max_speed: 400.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 167));
        let expected = 400.0 * MILLIMETERS_PER_INCH / 167.0;
        assert!((pen.max_speed - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn test_migrate_invalid_mm_max_speed() {
        let mut pen = Pen {
            max_speed: 300.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.max_speed, MILLIMETERS_PER_INCH);
    }

    #[test]
    fn test_migrate_legacy_max_speed_300_dpi() {
        let mut pen = Pen {
            max_speed: 3000.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_legacy_max_speed_200_dpi() {
        let mut pen = Pen {
            max_speed: 2000.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 200));
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_legacy_max_speed_167_dpi() {
        let mut pen = Pen {
            max_speed: 1670.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 167));
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_skips_mm_values() {
        let mut pen = Pen {
            max_speed: 254.0,
            ..Pen::default()
        };
        assert!(!migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.max_speed, 254.0);
        assert_eq!(pen.min_speed, 0.0);
    }

    #[test]
    fn test_migrate_skips_default_min_speed() {
        let mut pen = Pen {
            max_speed: 3000.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.min_speed, 0.0);
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_skips_mm_min_speed() {
        let mut pen = Pen {
            min_speed: 50.0,
            max_speed: 254.0,
            ..Pen::default()
        };
        assert!(!migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.min_speed, 50.0);
    }

    #[test]
    fn test_migrate_idempotent() {
        let mut pen = Pen {
            min_speed: 1000.0,
            max_speed: 3000.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.min_speed, 254.0 / 3.0);
        assert_eq!(pen.max_speed, 254.0);
        assert!(!migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.min_speed, 254.0 / 3.0);
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_legacy_min_speed() {
        let mut pen = Pen {
            min_speed: 1000.0,
            max_speed: 254.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 300));
        assert_eq!(pen.min_speed, 254.0 / 3.0);
        assert_eq!(pen.max_speed, 254.0);
    }

    #[test]
    fn test_migrate_legacy_min_speed_200_dpi() {
        let mut pen = Pen {
            min_speed: 1000.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 200));
        assert_eq!(pen.min_speed, 127.0);
    }

    #[test]
    fn test_migrate_legacy_min_speed_167_dpi() {
        let mut pen = Pen {
            min_speed: 835.0,
            ..Pen::default()
        };
        assert!(migrate_pen_speed_units_from_px(&mut pen, 167));
        assert_eq!(pen.min_speed, 127.0);
    }
}
