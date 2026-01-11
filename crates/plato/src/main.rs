mod app;

use crate::app::run;
use plato_core::anyhow::Error;

fn main() -> Result<(), Error> {
    run()?;
    Ok(())
}
