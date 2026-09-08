//! Integration tests ported from the former `session.rs` module.
//!
//! Exercises wake-lock timing, autosleep sysfs, soft-indicate, and grace
//! behaviour through [`Inhibitor::with_paths`](crate::device::inhibitor::Inhibitor::with_paths).

#[cfg(all(test, target_os = "linux"))]
mod session {
    use super::super::WAKE_LOCK_NAME;
    use super::super::paths::SoftSuspendPaths;
    use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
    use crate::device::leds::{DeviceLeds, LedsError};
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use crate::device::soft_suspend::mode::AutosleepMode;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::Duration;

    struct CountingLeds {
        on_calls: AtomicU32,
        off_calls: AtomicU32,
    }

    impl DeviceLeds for CountingLeds {
        fn on(&self) -> Result<(), LedsError> {
            self.on_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn off(&self) -> Result<(), LedsError> {
            self.off_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn temp_paths() -> (tempfile::TempDir, SoftSuspendPaths) {
        SoftSuspendPaths::test_fixture()
    }

    fn unlock_name(paths: &SoftSuspendPaths) -> String {
        fs::read_to_string(&paths.wake_unlock)
            .expect("read")
            .trim()
            .to_string()
    }

    fn make_unwritable(path: &std::path::Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms).expect("chmod");
    }

    fn wait_for<F: Fn() -> bool>(predicate: F) {
        for _ in 0..200 {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("condition not met within timeout");
    }

    #[test]
    fn first_acquire_writes_wake_lock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();

        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME
        );
        drop(lease);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn set_mode_writes_autosleep() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );

        inhibitor.set_mode(AutosleepMode::Freeze);

        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);
    }

    #[test]
    fn set_mode_keeps_previous_when_autosleep_write_fails() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Freeze);
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);

        make_unwritable(&paths.autosleep);
        inhibitor.set_mode(AutosleepMode::Mem);

        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
    }

    #[test]
    fn failed_wake_lock_write_does_not_claim_held() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_secs(60));
        make_unwritable(&paths.wake_lock);

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            ""
        );
        drop(lease);
        fs::write(&paths.wake_unlock, "").expect("clear unlock");

        inhibitor.set_autosleep_grace(Duration::from_millis(30));
        thread::sleep(Duration::from_millis(80));
        assert_eq!(
            unlock_name(&paths),
            "",
            "failed wake_lock must leave held=false so grace updates do not unlock"
        );
    }

    #[test]
    fn unsupported_mode_falls_back_to_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = SoftSuspendPaths {
            state: dir.path().join("state"),
            autosleep: dir.path().join("autosleep"),
            wake_lock: dir.path().join("wake_lock"),
            wake_unlock: dir.path().join("wake_unlock"),
        };
        fs::write(&paths.state, "mem\n").expect("state");
        fs::write(&paths.autosleep, "off\n").expect("autosleep");
        fs::write(&paths.wake_lock, "").expect("wake_lock");
        fs::write(&paths.wake_unlock, "").expect("wake_unlock");
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );

        inhibitor.set_mode(AutosleepMode::Freeze);

        assert_eq!(inhibitor.mode(), AutosleepMode::Off);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "off"
        );
    }

    #[test]
    fn led_indicator_on_while_armed_when_enabled() {
        let (_dir, paths) = temp_paths();
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            Some(leds.clone() as Arc<dyn DeviceLeds>),
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.apply_settings(AutosleepMode::Mem, true, Duration::ZERO);

        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);
        assert!(inhibitor.is_empty());
        let off_after_setup = leds.off_calls.load(Ordering::SeqCst);

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .unwrap();
        drop(lease);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
        assert_eq!(
            leds.off_calls.load(Ordering::SeqCst),
            off_after_setup,
            "soft-indicate must stay active while wake-lock holders churn"
        );

        inhibitor.set_indicate_autosleep_led(false);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) > off_after_setup);
    }

    #[test]
    fn nested_leases_keep_single_wake_lock_cycle() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);

        let a = inhibitor.acquire(Kind::SoftSuspend, "a").unwrap();
        let b = inhibitor.acquire(Kind::SoftSuspend, "b").unwrap();
        assert_eq!(inhibitor.len(), 2);
        drop(a);
        assert_eq!(unlock_name(&paths), "");
        drop(b);
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn grace_delays_wake_unlock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(80));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        drop(lease);

        assert_eq!(unlock_name(&paths), "");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn lease_during_grace_cancels_pending_unlock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(80));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        drop(lease);
        let _again = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        thread::sleep(Duration::from_millis(120));

        assert_eq!(unlock_name(&paths), "");
        assert!(inhibitor.has_holders());
    }

    #[test]
    fn reacquire_from_other_thread_during_grace_keeps_lock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(100));

        drop(
            inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
                .unwrap(),
        );

        let inhibitor = Arc::new(inhibitor);
        let inhibitor_thread = Arc::clone(&inhibitor);
        let join = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            inhibitor_thread
                .acquire(Kind::SoftSuspend, "worker")
                .unwrap()
        });
        let lease = join.join().expect("acquire thread");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(unlock_name(&paths), "");
        assert!(inhibitor.has_holders());
        drop(lease);
    }

    #[test]
    fn set_grace_while_empty_reschedules_deadline() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(50));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        drop(lease);
        inhibitor.set_autosleep_grace(Duration::from_millis(150));

        thread::sleep(Duration::from_millis(80));
        assert_eq!(unlock_name(&paths), "");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }

    #[test]
    fn repeated_empty_cycles_reuse_worker() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(40));

        for _ in 0..2 {
            fs::write(&paths.wake_unlock, "").expect("clear unlock");
            let lease = inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
                .unwrap();
            drop(lease);
            assert_eq!(unlock_name(&paths), "");
            thread::sleep(Duration::from_millis(80));
            assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
        }
    }

    #[test]
    fn drop_inhibitor_mid_grace_shuts_down_cleanly() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem);
        inhibitor.set_autosleep_grace(Duration::from_millis(200));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .unwrap();
        drop(lease);
        drop(inhibitor);

        thread::sleep(Duration::from_millis(50));
        assert_eq!(unlock_name(&paths), WAKE_LOCK_NAME);
    }
}
