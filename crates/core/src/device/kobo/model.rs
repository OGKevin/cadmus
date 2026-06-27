use crate::device::{DeviceCapabilities, DeviceIdentity, DeviceRotation, FrontlightKind};
use crate::input::TouchProto;
use std::fmt;

/// Kobo device model identifiers.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Model {
    Aura,
    AuraEd2V1,
    AuraEd2V2,
    AuraH2O,
    AuraH2OEd2V1,
    AuraH2OEd2V2,
    AuraHD,
    AuraONE,
    AuraONELimEd,
    Clara2E,
    ClaraBW,
    ClaraColour,
    ClaraHD,
    Elipsa,
    Elipsa2E,
    Forma,
    Forma32GB,
    Glo,
    GloHD,
    Libra2,
    LibraColour,
    LibraH2O,
    Mini,
    Nia,
    Sage,
    Touch2,
    TouchAB,
    TouchC,
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

impl Model {
    /// Creates a `Model` from product and model number strings.
    pub(crate) fn new(product: &str, model_number: &str) -> Model {
        match product {
            "kraken" => Model::Glo,
            "pixie" => Model::Mini,
            "dragon" => Model::AuraHD,
            "phoenix" => Model::Aura,
            "dahlia" => Model::AuraH2O,
            "alyssum" => Model::GloHD,
            "pika" => Model::Touch2,
            "daylight" => {
                if model_number == "381" {
                    Model::AuraONELimEd
                } else {
                    Model::AuraONE
                }
            }
            "star" => {
                if model_number == "379" {
                    Model::AuraEd2V2
                } else {
                    Model::AuraEd2V1
                }
            }
            "snow" => {
                if model_number == "378" {
                    Model::AuraH2OEd2V2
                } else {
                    Model::AuraH2OEd2V1
                }
            }
            "nova" => Model::ClaraHD,
            "frost" => {
                if model_number == "380" {
                    Model::Forma32GB
                } else {
                    Model::Forma
                }
            }
            "storm" => Model::LibraH2O,
            "luna" => Model::Nia,
            "europa" => Model::Elipsa,
            "cadmus" => Model::Sage,
            "io" => Model::Libra2,
            "goldfinch" => Model::Clara2E,
            "condor" => Model::Elipsa2E,
            "spaBW" | "spaBWTPV" => Model::ClaraBW,
            "spaColour" => Model::ClaraColour,
            "monza" => Model::LibraColour,
            _ => {
                if model_number == "320" {
                    Model::TouchC
                } else {
                    Model::TouchAB
                }
            }
        }
    }

    pub(super) fn gyro_rotation_fn(self) -> fn(i8) -> i8 {
        match self {
            Model::LibraH2O => |n| n ^ 1,
            Model::Libra2 | Model::Sage | Model::Elipsa2E | Model::LibraColour => |n| (6 - n) % 4,
            Model::Elipsa => |n| (4 - n) % 4,
            _ => |n| n,
        }
    }
}

impl DeviceIdentity for Model {
    fn model(&self) -> crate::device::Model {
        crate::device::Model::Kobo(*self)
    }

    fn proto(&self) -> TouchProto {
        match self {
            Model::Glo => TouchProto::Single,
            Model::Mini => TouchProto::Single,
            Model::AuraHD => TouchProto::Single,
            Model::Aura => TouchProto::MultiA,
            Model::AuraH2O => TouchProto::MultiA,
            Model::GloHD => TouchProto::MultiA,
            Model::Touch2 => TouchProto::MultiA,
            Model::AuraONELimEd | Model::AuraONE => TouchProto::MultiA,
            Model::AuraEd2V2 | Model::AuraEd2V1 => TouchProto::MultiA,
            Model::AuraH2OEd2V2 | Model::AuraH2OEd2V1 => TouchProto::MultiB,
            Model::ClaraHD => TouchProto::MultiB,
            Model::Forma32GB | Model::Forma => TouchProto::MultiB,
            Model::LibraH2O => TouchProto::MultiB,
            Model::Nia => TouchProto::MultiA,
            Model::Elipsa => TouchProto::MultiC,
            Model::Sage => TouchProto::MultiC,
            Model::Libra2 => TouchProto::MultiC,
            Model::Clara2E => TouchProto::MultiB,
            Model::Elipsa2E => TouchProto::MultiC,
            Model::ClaraBW | Model::ClaraColour => TouchProto::MultiB,
            Model::LibraColour => TouchProto::MultiB,
            Model::TouchC | Model::TouchAB => TouchProto::Single,
        }
    }

    fn dims(&self) -> (u32, u32) {
        match self {
            Model::Glo => (758, 1024),
            Model::Mini => (600, 800),
            Model::AuraHD => (1080, 1440),
            Model::Aura => (758, 1024),
            Model::AuraH2O => (1080, 1440),
            Model::GloHD => (1072, 1448),
            Model::Touch2 => (600, 800),
            Model::AuraONELimEd | Model::AuraONE => (1404, 1872),
            Model::AuraEd2V2 | Model::AuraEd2V1 => (758, 1024),
            Model::AuraH2OEd2V2 | Model::AuraH2OEd2V1 => (1080, 1440),
            Model::ClaraHD => (1072, 1448),
            Model::Forma32GB | Model::Forma => (1440, 1920),
            Model::LibraH2O => (1264, 1680),
            Model::Nia => (758, 1024),
            Model::Elipsa => (1404, 1872),
            Model::Sage => (1440, 1920),
            Model::Libra2 => (1264, 1680),
            Model::Clara2E => (1072, 1448),
            Model::Elipsa2E => (1404, 1872),
            Model::ClaraBW | Model::ClaraColour => (1072, 1448),
            Model::LibraColour => (1264, 1680),
            Model::TouchC | Model::TouchAB => (600, 800),
        }
    }

    fn dpi(&self) -> u16 {
        match self {
            Model::Glo => 212,
            Model::Mini => 200,
            Model::AuraHD => 265,
            Model::Aura => 212,
            Model::AuraH2O => 265,
            Model::GloHD => 300,
            Model::Touch2 => 167,
            Model::AuraONELimEd | Model::AuraONE => 300,
            Model::AuraEd2V2 | Model::AuraEd2V1 => 212,
            Model::AuraH2OEd2V2 | Model::AuraH2OEd2V1 => 265,
            Model::ClaraHD => 300,
            Model::Forma32GB | Model::Forma => 300,
            Model::LibraH2O => 300,
            Model::Nia => 212,
            Model::Elipsa => 227,
            Model::Sage => 300,
            Model::Libra2 => 300,
            Model::Clara2E => 300,
            Model::Elipsa2E => 227,
            Model::ClaraBW | Model::ClaraColour => 300,
            Model::LibraColour => 300,
            Model::TouchC | Model::TouchAB => 167,
        }
    }

    fn mark(&self) -> u8 {
        match self {
            Model::LibraColour => 13,
            Model::ClaraBW | Model::ClaraColour => 12,
            Model::Elipsa2E => 11,
            Model::Clara2E => 10,
            Model::Libra2 => 9,
            Model::Sage | Model::Elipsa => 8,
            Model::Nia
            | Model::LibraH2O
            | Model::Forma32GB
            | Model::Forma
            | Model::ClaraHD
            | Model::AuraH2OEd2V2
            | Model::AuraEd2V2 => 7,
            Model::AuraH2OEd2V1
            | Model::AuraEd2V1
            | Model::AuraONELimEd
            | Model::AuraONE
            | Model::Touch2
            | Model::GloHD => 6,
            Model::AuraH2O | Model::Aura => 5,
            Model::AuraHD | Model::Mini | Model::Glo | Model::TouchC => 4,
            Model::TouchAB => 3,
        }
    }
}

impl DeviceCapabilities for Model {
    fn frontlight_kind(&self) -> FrontlightKind {
        match self {
            Model::ClaraHD
            | Model::Forma
            | Model::Forma32GB
            | Model::LibraH2O
            | Model::Sage
            | Model::Libra2
            | Model::Clara2E
            | Model::Elipsa2E
            | Model::ClaraBW
            | Model::ClaraColour
            | Model::LibraColour => FrontlightKind::Premixed,
            Model::AuraONE | Model::AuraONELimEd | Model::AuraH2OEd2V1 | Model::AuraH2OEd2V2 => {
                FrontlightKind::Natural
            }
            _ => FrontlightKind::Standard,
        }
    }

    fn has_lightsensor(&self) -> bool {
        matches!(self, Model::AuraONE | Model::AuraONELimEd)
    }

    fn has_gyroscope(&self) -> bool {
        matches!(
            self,
            Model::Forma
                | Model::Forma32GB
                | Model::LibraH2O
                | Model::Elipsa
                | Model::Sage
                | Model::Libra2
                | Model::Elipsa2E
                | Model::LibraColour
        )
    }

    fn has_page_turn_buttons(&self) -> bool {
        matches!(
            self,
            Model::Forma
                | Model::Forma32GB
                | Model::LibraH2O
                | Model::Sage
                | Model::Libra2
                | Model::LibraColour
        )
    }

    fn has_power_cover(&self) -> bool {
        matches!(self, Model::Sage)
    }

    fn has_removable_storage(&self) -> bool {
        matches!(
            self,
            Model::AuraH2O
                | Model::Aura
                | Model::AuraHD
                | Model::Glo
                | Model::TouchAB
                | Model::TouchC
        )
    }

    fn color_samples(&self) -> usize {
        match self {
            Model::ClaraColour | Model::LibraColour => 3,
            _ => 1,
        }
    }
}

impl DeviceRotation for Model {
    fn startup_rotation(&self) -> i8 {
        match self {
            Model::LibraH2O => 0,
            Model::AuraH2OEd2V1
            | Model::Forma
            | Model::Forma32GB
            | Model::Sage
            | Model::Libra2
            | Model::Elipsa2E
            | Model::LibraColour => 1,
            _ => 3,
        }
    }

    fn mirroring_scheme(&self) -> (i8, i8) {
        match self {
            Model::AuraH2OEd2V1 | Model::LibraH2O | Model::Libra2 => (3, 1),
            Model::Sage => (0, 1),
            Model::AuraH2OEd2V2 => (0, -1),
            Model::Forma | Model::Forma32GB => (2, -1),
            _ => (2, 1),
        }
    }

    fn swapping_scheme(&self) -> i8 {
        match self {
            Model::LibraH2O => 0,
            _ => 1,
        }
    }

    fn transformed_rotation(&self, n: i8) -> i8 {
        match self {
            Model::AuraHD | Model::AuraH2O => n ^ 2,
            Model::AuraH2OEd2V2 | Model::Forma | Model::Forma32GB => (4 - n) % 4,
            _ => n,
        }
    }

    fn transformed_gyroscope_rotation(&self, n: i8) -> i8 {
        (*self).gyro_rotation_fn()(n)
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::input::TouchProto;

    mod identity {
        use super::*;
        use crate::device::DeviceIdentity;

        #[test]
        fn sage() {
            assert_eq!(Model::Sage.model(), crate::device::Model::Kobo(Model::Sage));
            assert_eq!(DeviceIdentity::dims(&Model::Sage), (1440, 1920));
            assert_eq!(DeviceIdentity::dpi(&Model::Sage), 300);
            assert_eq!(DeviceIdentity::proto(&Model::Sage), TouchProto::MultiC);
        }

        #[test]
        fn forma() {
            assert_eq!(DeviceIdentity::dims(&Model::Forma), (1440, 1920));
            assert_eq!(DeviceIdentity::dpi(&Model::Forma), 300);
            assert_eq!(DeviceIdentity::proto(&Model::Forma), TouchProto::MultiB);
        }

        #[test]
        fn forma_32gb() {
            assert_eq!(DeviceIdentity::dims(&Model::Forma32GB), (1440, 1920));
            assert_eq!(DeviceIdentity::dpi(&Model::Forma32GB), 300);
        }

        #[test]
        fn aura_one() {
            assert_eq!(DeviceIdentity::dims(&Model::AuraONE), (1404, 1872));
            assert_eq!(DeviceIdentity::dpi(&Model::AuraONE), 300);
            assert_eq!(DeviceIdentity::proto(&Model::AuraONE), TouchProto::MultiA);
        }

        #[test]
        fn aura_one_lim_ed() {
            assert_eq!(DeviceIdentity::dims(&Model::AuraONELimEd), (1404, 1872));
            assert_eq!(DeviceIdentity::dpi(&Model::AuraONELimEd), 300);
        }

        #[test]
        fn libra_colour() {
            assert_eq!(DeviceIdentity::dims(&Model::LibraColour), (1264, 1680));
            assert_eq!(DeviceIdentity::dpi(&Model::LibraColour), 300);
            assert_eq!(
                DeviceIdentity::proto(&Model::LibraColour),
                TouchProto::MultiB
            );
        }

        #[test]
        fn clara_hd() {
            assert_eq!(DeviceIdentity::dims(&Model::ClaraHD), (1072, 1448));
            assert_eq!(DeviceIdentity::dpi(&Model::ClaraHD), 300);
            assert_eq!(DeviceIdentity::proto(&Model::ClaraHD), TouchProto::MultiB);
        }

        #[test]
        fn glo() {
            assert_eq!(DeviceIdentity::dims(&Model::Glo), (758, 1024));
            assert_eq!(DeviceIdentity::dpi(&Model::Glo), 212);
            assert_eq!(DeviceIdentity::proto(&Model::Glo), TouchProto::Single);
        }

        #[test]
        fn touch_ab() {
            assert_eq!(DeviceIdentity::dims(&Model::TouchAB), (600, 800));
            assert_eq!(DeviceIdentity::dpi(&Model::TouchAB), 167);
            assert_eq!(DeviceIdentity::proto(&Model::TouchAB), TouchProto::Single);
        }

        #[test]
        fn touch_c() {
            assert_eq!(DeviceIdentity::dims(&Model::TouchC), (600, 800));
            assert_eq!(DeviceIdentity::dpi(&Model::TouchC), 167);
        }

        #[test]
        fn elipsa() {
            assert_eq!(DeviceIdentity::dims(&Model::Elipsa), (1404, 1872));
            assert_eq!(DeviceIdentity::dpi(&Model::Elipsa), 227);
            assert_eq!(DeviceIdentity::proto(&Model::Elipsa), TouchProto::MultiC);
        }

        #[test]
        fn libra_h2o() {
            assert_eq!(DeviceIdentity::dims(&Model::LibraH2O), (1264, 1680));
            assert_eq!(DeviceIdentity::dpi(&Model::LibraH2O), 300);
            assert_eq!(DeviceIdentity::proto(&Model::LibraH2O), TouchProto::MultiB);
        }

        #[test]
        fn aura_h2o_ed2_v1() {
            assert_eq!(DeviceIdentity::dims(&Model::AuraH2OEd2V1), (1080, 1440));
            assert_eq!(DeviceIdentity::dpi(&Model::AuraH2OEd2V1), 265);
            assert_eq!(
                DeviceIdentity::proto(&Model::AuraH2OEd2V1),
                TouchProto::MultiB
            );
        }

        #[test]
        fn aura_h2o_ed2_v2() {
            assert_eq!(DeviceIdentity::dims(&Model::AuraH2OEd2V2), (1080, 1440));
            assert_eq!(DeviceIdentity::dpi(&Model::AuraH2OEd2V2), 265);
            assert_eq!(
                DeviceIdentity::proto(&Model::AuraH2OEd2V2),
                TouchProto::MultiB
            );
        }

        #[test]
        fn mini() {
            assert_eq!(DeviceIdentity::dims(&Model::Mini), (600, 800));
            assert_eq!(DeviceIdentity::dpi(&Model::Mini), 200);
        }

        #[test]
        fn clara_bw() {
            assert_eq!(DeviceIdentity::dims(&Model::ClaraBW), (1072, 1448));
            assert_eq!(DeviceIdentity::dpi(&Model::ClaraBW), 300);
        }

        #[test]
        fn clara_colour() {
            assert_eq!(DeviceIdentity::dims(&Model::ClaraColour), (1072, 1448));
            assert_eq!(DeviceIdentity::dpi(&Model::ClaraColour), 300);
        }

        #[test]
        fn elipsa_2e() {
            assert_eq!(DeviceIdentity::dims(&Model::Elipsa2E), (1404, 1872));
            assert_eq!(DeviceIdentity::dpi(&Model::Elipsa2E), 227);
        }

        #[test]
        fn clara_2e() {
            assert_eq!(DeviceIdentity::dims(&Model::Clara2E), (1072, 1448));
            assert_eq!(DeviceIdentity::dpi(&Model::Clara2E), 300);
        }

        #[test]
        fn libra_2() {
            assert_eq!(DeviceIdentity::dims(&Model::Libra2), (1264, 1680));
            assert_eq!(DeviceIdentity::dpi(&Model::Libra2), 300);
        }
    }

    mod mark {
        use super::*;
        use crate::device::DeviceIdentity;

        #[test]
        fn values() {
            assert_eq!(DeviceIdentity::mark(&Model::LibraColour), 13);
            assert_eq!(DeviceIdentity::mark(&Model::ClaraBW), 12);
            assert_eq!(DeviceIdentity::mark(&Model::ClaraColour), 12);
            assert_eq!(DeviceIdentity::mark(&Model::Elipsa2E), 11);
            assert_eq!(DeviceIdentity::mark(&Model::Clara2E), 10);
            assert_eq!(DeviceIdentity::mark(&Model::Libra2), 9);
            assert_eq!(DeviceIdentity::mark(&Model::Sage), 8);
            assert_eq!(DeviceIdentity::mark(&Model::Elipsa), 8);
            assert_eq!(DeviceIdentity::mark(&Model::Nia), 7);
            assert_eq!(DeviceIdentity::mark(&Model::LibraH2O), 7);
            assert_eq!(DeviceIdentity::mark(&Model::Forma), 7);
            assert_eq!(DeviceIdentity::mark(&Model::Forma32GB), 7);
            assert_eq!(DeviceIdentity::mark(&Model::ClaraHD), 7);
            assert_eq!(DeviceIdentity::mark(&Model::AuraH2OEd2V2), 7);
            assert_eq!(DeviceIdentity::mark(&Model::AuraEd2V2), 7);
            assert_eq!(DeviceIdentity::mark(&Model::AuraH2OEd2V1), 6);
            assert_eq!(DeviceIdentity::mark(&Model::AuraEd2V1), 6);
            assert_eq!(DeviceIdentity::mark(&Model::AuraONE), 6);
            assert_eq!(DeviceIdentity::mark(&Model::AuraONELimEd), 6);
            assert_eq!(DeviceIdentity::mark(&Model::Touch2), 6);
            assert_eq!(DeviceIdentity::mark(&Model::GloHD), 6);
            assert_eq!(DeviceIdentity::mark(&Model::AuraH2O), 5);
            assert_eq!(DeviceIdentity::mark(&Model::Aura), 5);
            assert_eq!(DeviceIdentity::mark(&Model::AuraHD), 4);
            assert_eq!(DeviceIdentity::mark(&Model::Mini), 4);
            assert_eq!(DeviceIdentity::mark(&Model::Glo), 4);
            assert_eq!(DeviceIdentity::mark(&Model::TouchC), 4);
            assert_eq!(DeviceIdentity::mark(&Model::TouchAB), 3);
        }
    }

    mod capabilities {
        use super::*;
        use crate::device::DeviceCapabilities;

        #[test]
        fn frontlight_kind() {
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::ClaraHD),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Forma),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Sage),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Libra2),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Clara2E),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Elipsa2E),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::ClaraBW),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::ClaraColour),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::LibraColour),
                FrontlightKind::Premixed
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::AuraONE),
                FrontlightKind::Natural
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::AuraONELimEd),
                FrontlightKind::Natural
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::AuraH2OEd2V1),
                FrontlightKind::Natural
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::AuraH2OEd2V2),
                FrontlightKind::Natural
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Glo),
                FrontlightKind::Standard
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Aura),
                FrontlightKind::Standard
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::AuraHD),
                FrontlightKind::Standard
            );
            assert_eq!(
                DeviceCapabilities::frontlight_kind(&Model::Mini),
                FrontlightKind::Standard
            );
        }

        #[test]
        fn has_natural_light() {
            assert!(DeviceCapabilities::has_natural_light(&Model::Sage));
            assert!(DeviceCapabilities::has_natural_light(&Model::AuraONE));
            assert!(DeviceCapabilities::has_natural_light(&Model::ClaraHD));
            assert!(!DeviceCapabilities::has_natural_light(&Model::Glo));
            assert!(!DeviceCapabilities::has_natural_light(&Model::Aura));
        }

        #[test]
        fn has_lightsensor() {
            assert!(DeviceCapabilities::has_lightsensor(&Model::AuraONE));
            assert!(DeviceCapabilities::has_lightsensor(&Model::AuraONELimEd));
            assert!(!DeviceCapabilities::has_lightsensor(&Model::Sage));
            assert!(!DeviceCapabilities::has_lightsensor(&Model::ClaraHD));
            assert!(!DeviceCapabilities::has_lightsensor(&Model::Glo));
        }

        #[test]
        fn has_gyroscope() {
            assert!(DeviceCapabilities::has_gyroscope(&Model::Forma));
            assert!(DeviceCapabilities::has_gyroscope(&Model::Forma32GB));
            assert!(DeviceCapabilities::has_gyroscope(&Model::LibraH2O));
            assert!(DeviceCapabilities::has_gyroscope(&Model::Elipsa));
            assert!(DeviceCapabilities::has_gyroscope(&Model::Sage));
            assert!(DeviceCapabilities::has_gyroscope(&Model::Libra2));
            assert!(DeviceCapabilities::has_gyroscope(&Model::Elipsa2E));
            assert!(DeviceCapabilities::has_gyroscope(&Model::LibraColour));
            assert!(!DeviceCapabilities::has_gyroscope(&Model::AuraONE));
            assert!(!DeviceCapabilities::has_gyroscope(&Model::ClaraHD));
            assert!(!DeviceCapabilities::has_gyroscope(&Model::Glo));
        }

        #[test]
        fn has_page_turn_buttons() {
            assert!(DeviceCapabilities::has_page_turn_buttons(&Model::Forma));
            assert!(DeviceCapabilities::has_page_turn_buttons(&Model::Forma32GB));
            assert!(DeviceCapabilities::has_page_turn_buttons(&Model::LibraH2O));
            assert!(DeviceCapabilities::has_page_turn_buttons(&Model::Sage));
            assert!(DeviceCapabilities::has_page_turn_buttons(&Model::Libra2));
            assert!(DeviceCapabilities::has_page_turn_buttons(
                &Model::LibraColour
            ));
            assert!(!DeviceCapabilities::has_page_turn_buttons(&Model::AuraONE));
            assert!(!DeviceCapabilities::has_page_turn_buttons(&Model::ClaraHD));
            assert!(!DeviceCapabilities::has_page_turn_buttons(&Model::Elipsa));
        }

        #[test]
        fn has_power_cover() {
            assert!(DeviceCapabilities::has_power_cover(&Model::Sage));
            assert!(!DeviceCapabilities::has_power_cover(&Model::Forma));
            assert!(!DeviceCapabilities::has_power_cover(&Model::AuraONE));
            assert!(!DeviceCapabilities::has_power_cover(&Model::LibraColour));
        }

        #[test]
        fn has_removable_storage() {
            assert!(DeviceCapabilities::has_removable_storage(&Model::AuraH2O));
            assert!(DeviceCapabilities::has_removable_storage(&Model::Aura));
            assert!(DeviceCapabilities::has_removable_storage(&Model::AuraHD));
            assert!(DeviceCapabilities::has_removable_storage(&Model::Glo));
            assert!(DeviceCapabilities::has_removable_storage(&Model::TouchAB));
            assert!(DeviceCapabilities::has_removable_storage(&Model::TouchC));
            assert!(!DeviceCapabilities::has_removable_storage(&Model::Sage));
            assert!(!DeviceCapabilities::has_removable_storage(&Model::Forma));
            assert!(!DeviceCapabilities::has_removable_storage(&Model::ClaraHD));
        }

        #[test]
        fn color_samples() {
            assert_eq!(DeviceCapabilities::color_samples(&Model::ClaraColour), 3);
            assert_eq!(DeviceCapabilities::color_samples(&Model::LibraColour), 3);
            assert_eq!(DeviceCapabilities::color_samples(&Model::Sage), 1);
            assert_eq!(DeviceCapabilities::color_samples(&Model::Forma), 1);
            assert_eq!(DeviceCapabilities::color_samples(&Model::AuraONE), 1);
            assert_eq!(DeviceCapabilities::color_samples(&Model::Glo), 1);
        }
    }

    mod rotation {
        use super::*;
        use crate::device::{DeviceRotation, Orientation};

        #[test]
        fn canonical_rotation() {
            let forma = Model::Forma;
            let aura_one = Model::AuraONE;
            for n in 0..4 {
                assert_eq!(forma.to_native(forma.to_canonical(n)), n);
            }
            assert_eq!(aura_one.to_native(0), aura_one.startup_rotation());
            assert_eq!(
                forma.to_native(1) - forma.to_native(0),
                aura_one.to_native(2) - aura_one.to_native(3)
            );
        }

        #[test]
        fn canonical_rotation_round_trip_all_models() {
            let models = [
                Model::Sage,
                Model::Forma,
                Model::Forma32GB,
                Model::AuraONE,
                Model::AuraONELimEd,
                Model::LibraColour,
                Model::ClaraHD,
                Model::Glo,
                Model::TouchAB,
                Model::TouchC,
                Model::Elipsa,
                Model::Elipsa2E,
                Model::LibraH2O,
                Model::Libra2,
                Model::AuraH2OEd2V1,
                Model::AuraH2OEd2V2,
                Model::Aura,
                Model::AuraHD,
                Model::GloHD,
                Model::Mini,
                Model::Nia,
                Model::Clara2E,
                Model::ClaraBW,
                Model::ClaraColour,
                Model::Touch2,
                Model::AuraEd2V1,
                Model::AuraEd2V2,
                Model::AuraH2O,
            ];
            for model in &models {
                for n in 0..4i8 {
                    assert_eq!(
                        model.to_native(model.to_canonical(n)),
                        n,
                        "round trip failed for model {model:?} at n={n}"
                    );
                }
            }
        }

        #[test]
        fn startup_rotation() {
            assert_eq!(DeviceRotation::startup_rotation(&Model::LibraH2O), 0);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Sage), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Forma), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Forma32GB), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Libra2), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Elipsa2E), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::LibraColour), 1);
            assert_eq!(DeviceRotation::startup_rotation(&Model::AuraONE), 3);
            assert_eq!(DeviceRotation::startup_rotation(&Model::Glo), 3);
            assert_eq!(DeviceRotation::startup_rotation(&Model::ClaraHD), 3);
        }

        #[test]
        fn mirroring_scheme() {
            assert_eq!(
                DeviceRotation::mirroring_scheme(&Model::AuraH2OEd2V1),
                (3, 1)
            );
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::LibraH2O), (3, 1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::Libra2), (3, 1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::Sage), (0, 1));
            assert_eq!(
                DeviceRotation::mirroring_scheme(&Model::AuraH2OEd2V2),
                (0, -1)
            );
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::Forma), (2, -1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::Forma32GB), (2, -1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::AuraONE), (2, 1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::Glo), (2, 1));
            assert_eq!(DeviceRotation::mirroring_scheme(&Model::ClaraHD), (2, 1));
        }

        #[test]
        fn swapping_scheme() {
            assert!(Model::LibraH2O.should_swap_axes(0));
            assert!(!Model::LibraH2O.should_swap_axes(1));
            assert!(Model::LibraH2O.should_swap_axes(2));
            assert!(!Model::LibraH2O.should_swap_axes(3));

            assert!(!Model::Sage.should_swap_axes(0));
            assert!(Model::Sage.should_swap_axes(1));
            assert!(!Model::Sage.should_swap_axes(2));
            assert!(Model::Sage.should_swap_axes(3));

            assert!(!Model::Forma.should_swap_axes(0));
            assert!(Model::Forma.should_swap_axes(1));
        }

        #[test]
        fn transformed_rotation_is_self_inverse() {
            for model in [
                Model::Aura,
                Model::AuraEd2V1,
                Model::AuraEd2V2,
                Model::AuraH2O,
                Model::AuraH2OEd2V1,
                Model::AuraH2OEd2V2,
                Model::AuraHD,
                Model::AuraONE,
                Model::AuraONELimEd,
                Model::Clara2E,
                Model::ClaraBW,
                Model::ClaraColour,
                Model::ClaraHD,
                Model::Elipsa,
                Model::Elipsa2E,
                Model::Forma,
                Model::Forma32GB,
                Model::Glo,
                Model::GloHD,
                Model::Libra2,
                Model::LibraColour,
                Model::LibraH2O,
                Model::Mini,
                Model::Nia,
                Model::Sage,
                Model::Touch2,
                Model::TouchAB,
                Model::TouchC,
            ] {
                for n in 0..4 {
                    assert_eq!(
                        model.transformed_rotation(model.transformed_rotation(n)),
                        n,
                        "transformed_rotation should be self-inverse for {model:?} at {n}"
                    );
                }
            }
        }

        #[test]
        fn transformed_rotation() {
            assert_eq!(Model::AuraHD.transformed_rotation(0), 2);
            assert_eq!(Model::AuraHD.transformed_rotation(1), 3);
            assert_eq!(Model::AuraH2O.transformed_rotation(0), 2);

            assert_eq!(Model::AuraH2OEd2V2.transformed_rotation(0), 0);
            assert_eq!(Model::AuraH2OEd2V2.transformed_rotation(1), 3);
            assert_eq!(Model::Forma.transformed_rotation(0), 0);
            assert_eq!(Model::Forma.transformed_rotation(1), 3);

            assert_eq!(Model::Sage.transformed_rotation(0), 0);
            assert_eq!(Model::Sage.transformed_rotation(1), 1);
            assert_eq!(Model::AuraONE.transformed_rotation(0), 0);
        }

        #[test]
        fn transformed_gyroscope_rotation() {
            assert_eq!(Model::LibraH2O.transformed_gyroscope_rotation(0), 1);
            assert_eq!(Model::LibraH2O.transformed_gyroscope_rotation(1), 0);

            assert_eq!(Model::Libra2.transformed_gyroscope_rotation(0), 2);
            assert_eq!(Model::Libra2.transformed_gyroscope_rotation(1), 1);

            assert_eq!(Model::Sage.transformed_gyroscope_rotation(0), 2);
            assert_eq!(Model::Sage.transformed_gyroscope_rotation(1), 1);

            assert_eq!(Model::Elipsa.transformed_gyroscope_rotation(0), 0);
            assert_eq!(Model::Elipsa.transformed_gyroscope_rotation(1), 3);

            assert_eq!(Model::Forma.transformed_gyroscope_rotation(0), 0);
            assert_eq!(Model::Glo.transformed_gyroscope_rotation(0), 0);
        }

        #[test]
        fn orientation() {
            let sage = Model::Sage;
            assert_eq!(sage.orientation(0), Orientation::Landscape);
            assert_eq!(sage.orientation(1), Orientation::Portrait);
            assert_eq!(sage.orientation(2), Orientation::Landscape);
            assert_eq!(sage.orientation(3), Orientation::Portrait);

            let libra_h2o = Model::LibraH2O;
            assert_eq!(libra_h2o.orientation(0), Orientation::Portrait);
            assert_eq!(libra_h2o.orientation(1), Orientation::Landscape);
        }

        #[test]
        fn should_invert_buttons() {
            let sage = Model::Sage;
            let sr = sage.startup_rotation();
            let (_, dir) = sage.mirroring_scheme();
            assert!(sage.should_invert_buttons((4 + sr - dir) % 4));
            assert!(sage.should_invert_buttons((4 + sr - 2 * dir) % 4));
            assert!(!sage.should_invert_buttons(sr));
        }

        #[test]
        fn should_mirror_axes() {
            let forma = Model::Forma;
            let (mxy, dir) = forma.mirroring_scheme();
            let mx = (4 + (mxy + dir)) % 4;
            let my = (4 + (mxy - dir)) % 4;
            assert_eq!(forma.should_mirror_axes(mxy), (true, true));
            assert_eq!(forma.should_mirror_axes(mx), (true, false));
            assert_eq!(forma.should_mirror_axes(my), (false, true));
        }
    }
}
