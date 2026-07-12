//! Macros for platform device trait implementations.
//!
//! - [`forward_device_identity!`], [`forward_device_capabilities!`], and
//!   [`forward_device_rotation!`] forward static model properties from a nested
//!   field (for example `kobo::Device` → `kobo::Model`).
//! - [`impl_device_hardware!`] generates [`DeviceHardware`](crate::device::DeviceHardware)
//!   field accessors for devices that store subsystems in uniformly named fields.

/// Generates [`DeviceIdentity`](crate::device::DeviceIdentity) by forwarding to a nested field.
#[macro_export]
macro_rules! forward_device_identity {
    ($device:ty, $field:ident) => {
        impl $crate::device::DeviceIdentity for $device {
            fn model(&self) -> $crate::device::Model {
                $crate::device::DeviceIdentity::model(&self.$field)
            }

            fn proto(&self) -> $crate::input::TouchProto {
                $crate::device::DeviceIdentity::proto(&self.$field)
            }

            fn dims(&self) -> (u32, u32) {
                $crate::device::DeviceIdentity::dims(&self.$field)
            }

            fn dpi(&self) -> u16 {
                $crate::device::DeviceIdentity::dpi(&self.$field)
            }

            fn mark(&self) -> u8 {
                $crate::device::DeviceIdentity::mark(&self.$field)
            }
        }
    };
}

/// Generates [`DeviceCapabilities`](crate::device::DeviceCapabilities) by forwarding to a nested field.
#[macro_export]
macro_rules! forward_device_capabilities {
    ($device:ty, $field:ident) => {
        impl $crate::device::DeviceCapabilities for $device {
            fn frontlight_kind(&self) -> $crate::device::FrontlightKind {
                $crate::device::DeviceCapabilities::frontlight_kind(&self.$field)
            }

            fn has_lightsensor(&self) -> bool {
                $crate::device::DeviceCapabilities::has_lightsensor(&self.$field)
            }

            fn has_gyroscope(&self) -> bool {
                $crate::device::DeviceCapabilities::has_gyroscope(&self.$field)
            }

            fn has_page_turn_buttons(&self) -> bool {
                $crate::device::DeviceCapabilities::has_page_turn_buttons(&self.$field)
            }

            fn has_power_cover(&self) -> bool {
                $crate::device::DeviceCapabilities::has_power_cover(&self.$field)
            }

            fn has_removable_storage(&self) -> bool {
                $crate::device::DeviceCapabilities::has_removable_storage(&self.$field)
            }

            fn color_samples(&self) -> usize {
                $crate::device::DeviceCapabilities::color_samples(&self.$field)
            }
        }
    };
}

/// Generates [`DeviceRotation`](crate::device::DeviceRotation) by forwarding model hooks to a nested field.
///
/// `boot = $boot_field` names the runtime field used for
/// [`DeviceRotation::boot_transformed_rotation`](crate::device::DeviceRotation::boot_transformed_rotation).
#[macro_export]
macro_rules! forward_device_rotation {
    ($device:ty, $field:ident, boot = $boot_field:ident) => {
        impl $crate::device::DeviceRotation for $device {
            fn startup_rotation(&self) -> i8 {
                $crate::device::DeviceRotation::startup_rotation(&self.$field)
            }

            fn mirroring_scheme(&self) -> (i8, i8) {
                $crate::device::DeviceRotation::mirroring_scheme(&self.$field)
            }

            fn swapping_scheme(&self) -> i8 {
                $crate::device::DeviceRotation::swapping_scheme(&self.$field)
            }

            fn transformed_rotation(&self, n: i8) -> i8 {
                $crate::device::DeviceRotation::transformed_rotation(&self.$field, n)
            }

            fn boot_transformed_rotation(&self) -> i8 {
                self.$boot_field
            }

            fn transformed_gyroscope_rotation(&self, n: i8) -> i8 {
                $crate::device::DeviceRotation::transformed_gyroscope_rotation(&self.$field, n)
            }
        }
    };
}

/// Generates [`DeviceHardware`](crate::device::DeviceHardware) for devices with standard field names.
///
/// List associated types as `Name = Type` pairs. Optionally append `override` hooks:
/// `metadata`, `set_system_timezone`, or `refresh_framebuffer_from_kernel`.
///
/// ```ignore
/// crate::impl_device_hardware!(
///     Device,
///     Framebuffer = Pixmap,
///     Battery = FakeBattery,
///     Frontlight = LightLevels,
///     LightSensor = u16,
///     WifiManager = TestWifiManager,
///     UsbManager = TestUsbManager,
///     PowerManager = TestPowerManager,
///     Rtc = TestRtc,
///     override metadata_from metadata,
/// );
/// ```
#[macro_export]
macro_rules! impl_device_hardware {
    (
        $device:ty,
        $( $assoc:ident = $assoc_ty:ty ),+ $(,)?
    ) => {
        $crate::impl_device_hardware!(@impl $device; $( $assoc = $assoc_ty ),+;);
    };
    (
        $device:ty,
        $( $assoc:ident = $assoc_ty:ty ),+;
        override $( $hook:ident $arg:ident ),+ $(,)?
    ) => {
        $crate::impl_device_hardware!(@impl $device; $( $assoc = $assoc_ty ),+; $( $hook $arg ),+);
    };
    (
        @impl $device:ty;
        $( $assoc:ident = $assoc_ty:ty ),+;
        $( $hook:ident $arg:ident ),* $(,)?
    ) => {
        impl $crate::device::DeviceHardware for $device {
            $(
                type $assoc = $assoc_ty;
            )+

            fn framebuffer(&self) -> &Self::Framebuffer {
                &self.framebuffer
            }

            fn framebuffer_mut(&mut self) -> &mut Self::Framebuffer {
                &mut self.framebuffer
            }

            fn battery(&self) -> &Self::Battery {
                &self.battery
            }

            fn battery_mut(&mut self) -> &mut Self::Battery {
                &mut self.battery
            }

            fn frontlight(&self) -> &Self::Frontlight {
                &self.frontlight
            }

            fn frontlight_mut(&mut self) -> &mut Self::Frontlight {
                &mut self.frontlight
            }

            fn lightsensor(&self) -> &Self::LightSensor {
                &self.lightsensor
            }

            fn lightsensor_mut(&mut self) -> &mut Self::LightSensor {
                &mut self.lightsensor
            }

            fn wifi_manager(
                &self,
            ) -> Result<std::sync::Arc<Self::WifiManager>, $crate::device::wifi::WifiError> {
                Ok(self.wifi_manager.clone())
            }

            fn usb_manager(
                &self,
            ) -> Result<std::sync::Arc<Self::UsbManager>, $crate::device::usb::UsbError> {
                Ok(self.usb_manager.clone())
            }

            fn power_manager(
                &self,
            ) -> Result<std::sync::Arc<Self::PowerManager>, $crate::device::power::PowerError> {
                Ok(self.power_manager.clone())
            }

            fn rtc(&self) -> Result<std::sync::Arc<Self::Rtc>, anyhow::Error> {
                Ok(self.rtc.clone())
            }

            fn time_manager(
                &self,
            ) -> Result<&$crate::time_manager::TimeManager<Self::Rtc>, anyhow::Error> {
                Ok(&self.time_manager)
            }

            $(
                $crate::impl_device_hardware!(@hook $hook $arg);
            )*
        }
    };

    (@hook metadata_from $field:ident) => {
        fn metadata(
            &self,
        ) -> Result<
            &$crate::device::metadata::DeviceMetadata,
            $crate::device::error::DeviceError,
        > {
            Ok(&self.$field)
        }
    };

    (@hook set_system_timezone linux) => {
        fn set_system_timezone(&self, tz: chrono_tz::Tz) -> Result<(), anyhow::Error> {
            $crate::device::linux::set_system_timezone(tz)?;
            Ok(())
        }
    };

    (@hook refresh_framebuffer_from_kernel framebuffer) => {
        fn refresh_framebuffer_from_kernel(&mut self) {
            self.framebuffer.refresh_from_kernel();
        }
    };
}
