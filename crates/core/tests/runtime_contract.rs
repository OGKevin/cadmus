//! Off-runtime contract for [`cadmus_core::runtime`].
//!
//! Runs in its own process so a published process handle from other unit
//! tests cannot mask the panic.

#[test]
#[should_panic(expected = "cadmus runtime handle required")]
fn block_on_panics_without_a_process_runtime() {
    cadmus_core::runtime::block_on(async {});
}

#[test]
#[should_panic(expected = "cadmus runtime handle required")]
fn current_handle_panics_without_a_process_runtime() {
    let _ = cadmus_core::runtime::current_handle();
}
