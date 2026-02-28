//! Binary entry point for xtask.
//!
//! This is a thin wrapper around the library crate. The main logic lives in
//! [`xtask_lib`] (the library target).

use xtask_lib::run;

fn main() -> anyhow::Result<()> {
    run()
}
