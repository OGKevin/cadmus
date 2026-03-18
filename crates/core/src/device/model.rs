//! Device model definitions.

use std::fmt;

/// Kobo device model identifiers.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Model {
    LibraColour,
    ClaraColour,
    ClaraBW,
    Elipsa2E,
    Clara2E,
    Libra2,
    Sage,
    Elipsa,
    Nia,
    LibraH2O,
    Forma32GB,
    Forma,
    ClaraHD,
    AuraH2OEd2V2,
    AuraH2OEd2V1,
    AuraEd2V2,
    AuraEd2V1,
    AuraONELimEd,
    AuraONE,
    Touch2,
    GloHD,
    AuraH2O,
    Aura,
    AuraHD,
    Mini,
    Glo,
    TouchC,
    TouchAB,
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Model::LibraColour => write!(f, "Libra Colour"),
            Model::ClaraColour => write!(f, "Clara Colour"),
            Model::ClaraBW => write!(f, "Clara BW"),
            Model::Elipsa2E => write!(f, "Elipsa 2E"),
            Model::Clara2E => write!(f, "Clara 2E"),
            Model::Libra2 => write!(f, "Libra 2"),
            Model::Sage => write!(f, "Sage"),
            Model::Elipsa => write!(f, "Elipsa"),
            Model::Nia => write!(f, "Nia"),
            Model::LibraH2O => write!(f, "Libra H₂O"),
            Model::Forma32GB => write!(f, "Forma 32GB"),
            Model::Forma => write!(f, "Forma"),
            Model::ClaraHD => write!(f, "Clara HD"),
            Model::AuraH2OEd2V1 => write!(f, "Aura H₂O Edition 2 Version 1"),
            Model::AuraH2OEd2V2 => write!(f, "Aura H₂O Edition 2 Version 2"),
            Model::AuraEd2V1 => write!(f, "Aura Edition 2 Version 1"),
            Model::AuraEd2V2 => write!(f, "Aura Edition 2 Version 2"),
            Model::AuraONELimEd => write!(f, "Aura ONE Limited Edition"),
            Model::AuraONE => write!(f, "Aura ONE"),
            Model::Touch2 => write!(f, "Touch 2.0"),
            Model::GloHD => write!(f, "Glo HD"),
            Model::AuraH2O => write!(f, "Aura H₂O"),
            Model::Aura => write!(f, "Aura"),
            Model::AuraHD => write!(f, "Aura HD"),
            Model::Mini => write!(f, "Mini"),
            Model::Glo => write!(f, "Glo"),
            Model::TouchC => write!(f, "Touch C"),
            Model::TouchAB => write!(f, "Touch A/B"),
        }
    }
}
