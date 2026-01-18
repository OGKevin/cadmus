mod app;

use crate::app::run;
use cadmus_core::anyhow::Error;

fn main() -> Result<(), Error> {
    run()?;
    Ok(())
}
