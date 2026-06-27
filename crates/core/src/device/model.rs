//! Device model definitions.

// #[cfg(all(feature = "kobo", not(test)))]
#[cfg(any(feature = "kobo", docsrs))]
use crate::device::kobo;

use crate::device::AppDevice;
use std::fmt;

/// Kobo device model identifiers.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Model {
    #[cfg(any(feature = "kobo", docsrs))]
    Kobo(kobo::Model),
    #[cfg(any(feature = "emulator", docsrs))]
    Emulator,
    TestDevice,
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            #[cfg(any(feature = "kobo", docsrs))]
            Model::Kobo(m) => {
                write!(f, "{}", m)
            }
            #[cfg(any(feature = "emulator", docsrs))]
            Model::Emulator => {
                write!(f, "Emulator")
            }
            Model::TestDevice => {
                write!(f, "TestDevice")
            }
        }
    }
}

impl Model {
    /// Creates a `Model` from product and model number strings.
    pub fn new(product: &str, model_number: &str) -> Model {
        cfg_select! {
            all(feature = "kobo", not(test)) => {
                Model::Kobo(kobo::Model::new(product, model_number))
            }
            _ => {
                panic!("Model::new is not implemented for this platform: {} {}", product, model_number)
            }
        }
    }

    /// Creates a `KoboDevice` for this model with the correct hardware properties.
    pub fn device(self) -> anyhow::Result<AppDevice> {
        match self {
            #[cfg(all(feature = "kobo", not(test)))]
            Model::Kobo(m) => m.device(),
            #[cfg(all(feature = "kobo", test))]
            Model::Kobo(_) => {
                panic!(
                    "Model::device is not implemented for this platform: {}",
                    self
                );
            }
            #[cfg(feature = "emulator")]
            Model::Emulator => {
                unimplemented!()
            }
            Model::TestDevice => {
                unimplemented!("TestDevice shall be created in tests")
            }
        }
    }
}
