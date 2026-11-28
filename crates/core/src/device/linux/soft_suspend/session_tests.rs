//! Integration tests ported from the former `session.rs` module.
//!
//! Exercises wake-lock timing, autosleep sysfs, soft-indicate, and grace
//! behaviour through [`Inhibitor::with_paths`](crate::device::inhibitor::Inhibitor::with_paths).

#[cfg(all(test, target_os = "linux"))]
mod session {
    use super::super::WAKE_LOCK_NAME;
    use super::super::paths::{
        SoftSuspendPaths, SysfsWrite, SysfsWriteError, write_sysfs_blocking,
    };
    use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
    use crate::device::leds::{DeviceLeds, LedsError};
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use crate::device::soft_suspend::mode::AutosleepMode;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

    async fn unlock_name(paths: &SoftSuspendPaths) -> String {
        tokio::fs::read_to_string(&paths.wake_unlock)
            .await
            .expect("read")
            .trim()
            .to_string()
    }

    async fn wait_for_unlock(paths: &SoftSuspendPaths) {
        for _ in 0..200 {
            if unlock_name(paths).await == WAKE_LOCK_NAME {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not met within timeout");
    }

    fn inhibitor_with_writer_result(
        paths: &SoftSuspendPaths,
        writer: impl Fn(&std::path::Path, &str) -> Result<SysfsWrite, SysfsWriteError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Inhibitor> {
        Inhibitor::with_paths_and_writer(
            paths.clone(),
            None,
            Arc::new(crate::device::battery::FakeBattery::new()),
            Some(Arc::new(writer)),
        )
    }

    fn make_unwritable(path: &std::path::Path) {
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms).expect("chmod");
    }

    async fn wait_for<F: Fn() -> bool>(predicate: F) {
        for _ in 0..200 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not met within timeout");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn first_acquire_writes_wake_lock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();

        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME
        );
        drop(lease);
        wait_for_unlock(&paths).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_mode_writes_autosleep() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );

        inhibitor.set_mode(AutosleepMode::Freeze).await.ok();

        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_mode_keeps_previous_when_autosleep_write_fails() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor
            .set_mode(AutosleepMode::Freeze)
            .await
            .expect("initial freeze write succeeds");
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);

        make_unwritable(&paths.autosleep);
        let result = inhibitor.set_mode(AutosleepMode::Mem).await;

        assert!(result.is_err());
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "freeze"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_mode_errors_and_keeps_previous_when_autosleep_node_missing() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor
            .set_mode(AutosleepMode::Freeze)
            .await
            .expect("initial freeze write succeeds");
        fs::remove_file(&paths.autosleep).expect("remove autosleep node");

        let result = inhibitor.set_mode(AutosleepMode::Mem).await;

        let error = result.expect_err("missing node must be an error");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(inhibitor.mode(), AutosleepMode::Freeze);
        assert!(!paths.autosleep.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_wake_lock_write_does_not_claim_held() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_secs(60));
        make_unwritable(&paths.wake_lock);

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            ""
        );
        drop(lease);
        fs::write(&paths.wake_unlock, "").expect("clear unlock");

        inhibitor.set_autosleep_grace(Duration::from_millis(30));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            unlock_name(&paths).await,
            "",
            "failed wake_lock must leave held=false so grace updates do not unlock"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsupported_mode_falls_back_to_off() {
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

        inhibitor.set_mode(AutosleepMode::Freeze).await.ok();

        assert_eq!(inhibitor.mode(), AutosleepMode::Off);
        assert_eq!(
            fs::read_to_string(&paths.autosleep).expect("read").trim(),
            "off"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn led_indicator_on_while_armed_when_enabled() {
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
        inhibitor
            .apply_settings(AutosleepMode::Mem, true, Duration::ZERO)
            .await
            .ok();

        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;
        assert!(inhibitor.is_empty());
        let off_after_setup = leds.off_calls.load(Ordering::SeqCst);

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .await
            .unwrap();
        drop(lease);
        wait_for_unlock(&paths).await;
        assert_eq!(
            leds.off_calls.load(Ordering::SeqCst),
            off_after_setup,
            "soft-indicate must stay active while wake-lock holders churn"
        );

        inhibitor.set_indicate_autosleep_led(false);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) > off_after_setup).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nested_leases_keep_single_wake_lock_cycle() {
        let (_dir, paths) = temp_paths();
        let (unlock_tx, mut unlock_rx) = tokio::sync::mpsc::unbounded_channel();
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if path.ends_with("wake_unlock") {
                let _ = unlock_tx.send(());
            }
            write_sysfs_blocking(path, value)
        });
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();

        let a = inhibitor.acquire(Kind::SoftSuspend, "a").await.unwrap();
        let b = inhibitor.acquire(Kind::SoftSuspend, "b").await.unwrap();
        assert_eq!(inhibitor.len(), 2);
        drop(a);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), unlock_rx.recv())
                .await
                .is_err(),
            "releasing one of two holders must not attempt wake_unlock"
        );
        assert_eq!(unlock_name(&paths).await, "");
        drop(b);
        tokio::time::timeout(Duration::from_secs(5), unlock_rx.recv())
            .await
            .expect("last release must attempt wake_unlock")
            .expect("unlock signal");
        wait_for_unlock(&paths).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grace_delays_wake_unlock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(80));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        drop(lease);

        assert_eq!(unlock_name(&paths).await, "");
        wait_for_unlock(&paths).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lease_during_grace_cancels_pending_unlock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(80));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        drop(lease);
        let _again = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;

        assert_eq!(unlock_name(&paths).await, "");
        assert!(inhibitor.has_holders());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reacquire_from_other_thread_during_grace_keeps_lock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(100));

        drop(
            inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
                .await
                .unwrap(),
        );

        let inhibitor = Arc::new(inhibitor);
        let inhibitor_thread = Arc::clone(&inhibitor);
        let join = crate::runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            inhibitor_thread
                .acquire(Kind::SoftSuspend, "worker")
                .await
                .unwrap()
        });
        let lease = join.await.expect("acquire task");
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(unlock_name(&paths).await, "");
        assert!(inhibitor.has_holders());
        drop(lease);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_grace_while_empty_reschedules_deadline() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(50));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        drop(lease);
        inhibitor.set_autosleep_grace(Duration::from_millis(150));

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(unlock_name(&paths).await, "");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(unlock_name(&paths).await, WAKE_LOCK_NAME);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_empty_cycles_reuse_worker() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(40));

        for _ in 0..2 {
            fs::write(&paths.wake_unlock, "").expect("clear unlock");
            let lease = inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
                .await
                .unwrap();
            drop(lease);
            assert_eq!(unlock_name(&paths).await, "");
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(unlock_name(&paths).await, WAKE_LOCK_NAME);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_inhibitor_mid_grace_shuts_down_cleanly() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_mode(AutosleepMode::Mem).await.ok();
        inhibitor.set_autosleep_grace(Duration::from_millis(200));

        let lease = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            .await
            .unwrap();
        drop(lease);
        drop(inhibitor);

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(unlock_name(&paths).await, WAKE_LOCK_NAME);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pause_unlocks_immediately_with_live_holder_and_grace() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_autosleep_grace(Duration::from_secs(60));
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();

        inhibitor.pause_soft_suspend_wake_lock().await;

        assert_eq!(unlock_name(&paths).await, WAKE_LOCK_NAME);
        assert_eq!(inhibitor.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_while_paused_is_tracked_and_locked_on_resume() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");

        let _wifi = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .await
            .unwrap();

        assert_eq!(inhibitor.len(), 2);
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            ""
        );
        inhibitor.resume_soft_suspend_wake_lock().await;
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            WAKE_LOCK_NAME
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn first_acquire_while_paused_tracks_and_locks_on_resume() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");

        let _wifi = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .await
            .unwrap();

        assert_eq!(inhibitor.len(), 1);
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            ""
        );
        inhibitor.resume_soft_suspend_wake_lock().await;
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            WAKE_LOCK_NAME
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_with_no_live_holders_leaves_wake_lock_unlocked() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");
        tokio::fs::write(&paths.wake_unlock, "")
            .await
            .expect("clear unlock");

        drop(thumbnail);
        inhibitor.resume_soft_suspend_wake_lock().await;

        assert!(inhibitor.is_empty());
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            ""
        );
        assert_eq!(unlock_name(&paths).await, "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_pause_and_resume_are_idempotent() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();

        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_unlock, "")
            .await
            .expect("clear unlock");
        inhibitor.pause_soft_suspend_wake_lock().await;
        assert_eq!(unlock_name(&paths).await, "");

        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");
        inhibitor.resume_soft_suspend_wake_lock().await;
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            WAKE_LOCK_NAME
        );
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");
        inhibitor.resume_soft_suspend_wake_lock().await;
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            ""
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn last_holder_drop_while_paused_stays_unlocked_after_resume() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        inhibitor.set_autosleep_grace(Duration::from_millis(30));
        let thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");
        tokio::fs::write(&paths.wake_unlock, "")
            .await
            .expect("clear unlock");

        drop(thumbnail);
        inhibitor.resume_soft_suspend_wake_lock().await;

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            ""
        );
        assert_eq!(unlock_name(&paths).await, "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_inhibit_blocks_pause() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let _full = inhibitor.acquire(Kind::Full, "ota").await.unwrap();
        tokio::fs::write(&paths.wake_unlock, "")
            .await
            .expect("clear unlock");

        let outcome = inhibitor.pause_soft_suspend_wake_lock().await;

        assert_eq!(
            outcome,
            crate::device::inhibitor::WakeLockPauseOutcome::BlockedByFull
        );
        assert_eq!(unlock_name(&paths).await, "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_inhibit_acquired_while_paused_restores_wake_lock() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        inhibitor.pause_soft_suspend_wake_lock().await;
        tokio::fs::write(&paths.wake_lock, "")
            .await
            .expect("clear lock");

        let _full = inhibitor.acquire(Kind::Full, "ota").await.unwrap();

        assert!(inhibitor.full_active());
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            WAKE_LOCK_NAME
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_lease_during_suppression_ends_window_and_blocks_cycle() {
        let (_dir, paths) = temp_paths();
        let inhibitor = Inhibitor::with_paths(
            paths.clone(),
            None,
            std::sync::Arc::new(crate::device::battery::FakeBattery::new()),
        );
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        let crate::device::inhibitor::WakeLockPauseOutcome::Paused(epoch) =
            inhibitor.pause_soft_suspend_wake_lock().await
        else {
            panic!("the sleep window must suppress the kernel lock");
        };
        assert!(inhibitor.wake_lock_epoch_is_current(epoch));

        let full = inhibitor.acquire(Kind::Full, "ota").await.unwrap();

        assert!(
            !inhibitor.wake_lock_epoch_is_current(epoch),
            "a Full lease ends the window, so a poll reports the cycle blocked"
        );
        assert_eq!(
            tokio::fs::read_to_string(&paths.wake_lock)
                .await
                .expect("read")
                .trim(),
            WAKE_LOCK_NAME,
            "cadmus returns while the Full lease is held"
        );

        drop(full);
        let replayed = inhibitor.pause_soft_suspend_wake_lock().await;

        assert!(
            matches!(
                replayed,
                crate::device::inhibitor::WakeLockPauseOutcome::Paused(next) if next != epoch
            ),
            "the replayed cycle opens its own suppression window"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_acquisition_fails_when_kernel_lock_cannot_be_written() {
        let (_dir, paths) = temp_paths();
        let inhibitor = inhibitor_with_writer_result(&paths, |path, _value| {
            Err(SysfsWriteError::missing(path))
        });
        let result = inhibitor.acquire(Kind::Full, "ota").await;

        assert!(matches!(
            result,
            Err(crate::device::inhibitor::InhibitorError::WakeLockWriteFailed)
        ));
        assert!(!inhibitor.full_active());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pause_returns_blocked_after_transient_full_interruption() {
        let (_dir, paths) = temp_paths();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
        let barrier_used = Arc::new(AtomicBool::new(false));
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if path.ends_with("wake_unlock") && !barrier_used.swap(true, Ordering::SeqCst) {
                started_tx.send(()).expect("signal unlock start");
                release_rx
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("wait for unlock release");
                Ok(SysfsWrite::Written)
            } else {
                write_sysfs_blocking(path, value)
            }
        });
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        let pausing = Arc::clone(&inhibitor);
        let pause = tokio::spawn(async move { pausing.pause_soft_suspend_wake_lock().await });

        tokio::time::timeout(
            Duration::from_secs(5),
            crate::runtime::spawn_blocking(move || started_rx.recv()),
        )
        .await
        .expect("unlock barrier start")
        .expect("unlock worker join")
        .expect("unlock started");

        let full_backend = Arc::clone(&inhibitor);
        let full =
            crate::runtime::spawn(async move { full_backend.acquire(Kind::Full, "ota").await });
        wait_for(|| inhibitor.full_active()).await;
        release_tx.send(()).expect("release unlock barrier");
        let full = full.await.expect("full worker join").unwrap();
        drop(full);

        assert_eq!(
            pause.await.expect("pause join"),
            crate::device::inhibitor::WakeLockPauseOutcome::BlockedByFull,
            "a Full interruption must never report Paused, even after release"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_restore_retries_without_another_api_call() {
        let (_dir, paths) = temp_paths();
        let attempts = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&attempts);
        let fail = Arc::new(AtomicU32::new(0));
        let failing = Arc::clone(&fail);
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if failing.load(Ordering::SeqCst) == 1 && path.ends_with("wake_lock") {
                let attempt = counting.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt < 3 {
                    Err(SysfsWriteError::missing(path))
                } else {
                    write_sysfs_blocking(path, value)
                }
            } else {
                write_sysfs_blocking(path, value)
            }
        });
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        assert!(matches!(
            inhibitor.pause_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockPauseOutcome::Paused(_)
        ));
        fs::write(&paths.wake_lock, "").expect("clear lock");
        fail.store(1, Ordering::SeqCst);

        assert_eq!(
            inhibitor.resume_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockResumeOutcome::RestoreFailed
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while fs::read_to_string(&paths.wake_lock)
            .expect("read wake lock")
            .trim()
            != WAKE_LOCK_NAME
        {
            assert!(std::time::Instant::now() < deadline, "retry never restored");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(attempts.load(Ordering::SeqCst) >= 3);
        drop(_thumbnail);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restore_retry_exhaustion_stops_and_explicit_resume_recovers() {
        let (_dir, paths) = temp_paths();
        let attempts = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&attempts);
        let fail = Arc::new(AtomicU32::new(0));
        let failing = Arc::clone(&fail);
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if failing.load(Ordering::SeqCst) == 1 && path.ends_with("wake_lock") {
                counting.fetch_add(1, Ordering::SeqCst);
                Err(SysfsWriteError::missing(path))
            } else {
                write_sysfs_blocking(path, value)
            }
        });
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        let _ = inhibitor.pause_soft_suspend_wake_lock().await;
        fs::write(&paths.wake_lock, "").expect("clear lock");
        fail.store(1, Ordering::SeqCst);

        assert_eq!(
            inhibitor.resume_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockResumeOutcome::RestoreFailed
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::SeqCst) < 4 {
            assert!(
                std::time::Instant::now() < deadline,
                "retry budget did not run to completion"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            4,
            "exhausted retry budget must stop consuming attempts"
        );
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            "",
            "failed restore must not claim a written lock"
        );

        fail.store(0, Ordering::SeqCst);
        assert_eq!(
            inhibitor.resume_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockResumeOutcome::Restored
        );
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn acquire_waits_out_an_in_flight_unlock() {
        let (_dir, paths) = temp_paths();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
        let barrier_used = Arc::new(AtomicBool::new(false));
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if path.ends_with("wake_unlock") && !barrier_used.swap(true, Ordering::SeqCst) {
                started_tx.send(()).expect("signal unlock start");
                release_rx
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("wait for unlock release");
                Ok(SysfsWrite::Written)
            } else {
                write_sysfs_blocking(path, value)
            }
        });
        let lease = inhibitor.acquire(Kind::SoftSuspend, "first").await.unwrap();
        fs::write(&paths.wake_lock, "").expect("clear lock");
        let drop_lease = crate::runtime::spawn_blocking(move || drop(lease));
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::runtime::spawn_blocking(move || started_rx.recv()),
        )
        .await
        .expect("unlock barrier start")
        .expect("unlock worker join")
        .expect("unlock started");

        let acquiring = Arc::clone(&inhibitor);
        let next =
            crate::runtime::spawn(
                async move { acquiring.acquire(Kind::SoftSuspend, "next").await },
            );
        wait_for(|| inhibitor.has_holders()).await;
        release_tx.send(()).expect("release unlock barrier");
        let _next = next.await.expect("acquire worker join").unwrap();
        drop_lease.await.expect("drop worker join");
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME,
            "acquisition must end with confirmed wake-lock protection"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grace_change_does_not_cancel_failed_restore_retries() {
        let (_dir, paths) = temp_paths();
        let attempts = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&attempts);
        let fail = Arc::new(AtomicU32::new(0));
        let failing = Arc::clone(&fail);
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if failing.load(Ordering::SeqCst) == 1 && path.ends_with("wake_lock") {
                let attempt = counting.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt < 3 {
                    Err(SysfsWriteError::missing(path))
                } else {
                    write_sysfs_blocking(path, value)
                }
            } else {
                write_sysfs_blocking(path, value)
            }
        });
        let _thumbnail = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            .await
            .unwrap();
        assert!(matches!(
            inhibitor.pause_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockPauseOutcome::Paused(_)
        ));
        fs::write(&paths.wake_lock, "").expect("clear lock");
        fail.store(1, Ordering::SeqCst);

        assert_eq!(
            inhibitor.resume_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockResumeOutcome::RestoreFailed
        );
        inhibitor.set_autosleep_grace(Duration::from_millis(30));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::SeqCst) < 3
            || fs::read_to_string(&paths.wake_lock)
                .expect("read wake lock")
                .trim()
                != WAKE_LOCK_NAME
        {
            assert!(
                std::time::Instant::now() < deadline,
                "grace change cancelled restore retries"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn last_release_during_in_flight_restore_keeps_configured_grace() {
        let grace = Duration::from_millis(400);
        let (_dir, paths) = temp_paths();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let armed = Arc::new(AtomicBool::new(false));
        let armed_writer = Arc::clone(&armed);
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if path.ends_with("wake_lock") && armed_writer.swap(false, Ordering::SeqCst) {
                started_tx.send(()).expect("signal restore start");
                release_rx
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("wait for restore release");
            }
            write_sysfs_blocking(path, value)
        });
        inhibitor.set_autosleep_grace(grace);

        let holder = inhibitor
            .acquire(Kind::SoftSuspend, "holder")
            .await
            .unwrap();
        assert!(matches!(
            inhibitor.pause_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockPauseOutcome::Paused(_)
        ));
        fs::write(&paths.wake_lock, "").expect("clear lock");
        fs::write(&paths.wake_unlock, "").expect("clear unlocked state");
        armed.store(true, Ordering::SeqCst);

        let resuming = Arc::clone(&inhibitor);
        let resume = tokio::spawn(async move { resuming.resume_soft_suspend_wake_lock().await });
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::runtime::spawn_blocking(move || started_rx.recv()),
        )
        .await
        .expect("restore barrier start")
        .expect("restore worker join")
        .expect("restore started");

        let released_at = std::time::Instant::now();
        drop(holder);
        release_tx.send(()).expect("release restore barrier");

        let outcome = tokio::time::timeout(Duration::from_secs(5), resume)
            .await
            .expect("resume completes")
            .expect("resume join");
        assert_eq!(
            outcome,
            crate::device::inhibitor::WakeLockResumeOutcome::Restored
        );
        assert_eq!(
            fs::read_to_string(&paths.wake_lock).expect("read").trim(),
            WAKE_LOCK_NAME
        );
        assert_eq!(
            unlock_name(&paths).await,
            "",
            "grace must survive a release that raced the in-flight restore"
        );

        wait_for_unlock(&paths).await;
        assert!(
            released_at.elapsed() >= grace,
            "unlock must not precede the configured grace"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grace_update_during_in_flight_restore_arms_new_grace() {
        let grace = Duration::from_millis(400);
        let (_dir, paths) = temp_paths();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let armed = Arc::new(AtomicBool::new(false));
        let armed_writer = Arc::clone(&armed);
        let inhibitor = inhibitor_with_writer_result(&paths, move |path, value| {
            if path.ends_with("wake_lock") && armed_writer.swap(false, Ordering::SeqCst) {
                started_tx.send(()).expect("signal restore start");
                release_rx
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("wait for restore release");
            }
            write_sysfs_blocking(path, value)
        });

        let holder = inhibitor
            .acquire(Kind::SoftSuspend, "holder")
            .await
            .unwrap();
        assert!(matches!(
            inhibitor.pause_soft_suspend_wake_lock().await,
            crate::device::inhibitor::WakeLockPauseOutcome::Paused(_)
        ));
        fs::write(&paths.wake_lock, "").expect("clear lock");
        fs::write(&paths.wake_unlock, "").expect("clear unlocked state");
        armed.store(true, Ordering::SeqCst);

        let resuming = Arc::clone(&inhibitor);
        let resume = tokio::spawn(async move { resuming.resume_soft_suspend_wake_lock().await });
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::runtime::spawn_blocking(move || started_rx.recv()),
        )
        .await
        .expect("restore barrier start")
        .expect("restore worker join")
        .expect("restore started");

        drop(holder);
        let updated_at = std::time::Instant::now();
        inhibitor.set_autosleep_grace(grace);
        release_tx.send(()).expect("release restore barrier");

        let outcome = tokio::time::timeout(Duration::from_secs(5), resume)
            .await
            .expect("resume completes")
            .expect("resume join");
        assert_eq!(
            outcome,
            crate::device::inhibitor::WakeLockResumeOutcome::Restored
        );
        assert_eq!(
            unlock_name(&paths).await,
            "",
            "a grace update during an in-flight restore must arm the new grace"
        );

        wait_for_unlock(&paths).await;
        assert!(
            updated_at.elapsed() >= grace,
            "unlock must not precede the updated grace"
        );
    }
}
