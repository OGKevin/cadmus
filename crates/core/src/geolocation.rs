use anyhow::Error;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

use crate::http::Client as HttpClient;

/// Geographic coordinates expressed as latitude and longitude in degrees.
///
/// Latitude must be in the inclusive range `-90.0..=90.0` and longitude
/// must be in the inclusive range `-180.0..=180.0`.
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coordinates(f64, f64);

impl Coordinates {
    /// Creates validated geographic coordinates.
    ///
    /// Returns an error if either component is `NaN` or falls outside the
    /// allowed latitude/longitude bounds.
    pub fn new(lat: f64, lon: f64) -> Result<Self, anyhow::Error> {
        anyhow::ensure!(
            !lat.is_nan()
                && !lon.is_nan()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon),
            "The given coordinates are invalid"
        );

        Ok(Self(lat, lon))
    }

    /// Returns the latitude in degrees.
    pub fn latitude(self) -> f64 {
        self.0
    }

    /// Returns the longitude in degrees.
    pub fn longitude(self) -> f64 {
        self.1
    }
}

impl From<Coordinates> for sunrise::Coordinates {
    fn from(value: Coordinates) -> Self {
        Self::new(value.0, value.1).unwrap_or_else(|| {
           panic!(
               "Given coordinates are invalid, the Coordinates struct should not be holding invalid coordinates: {value}"
           )
       })
    }
}

impl fmt::Display for Coordinates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({},{})", self.latitude(), self.longitude())
    }
}

/// A geographic location paired with its local time zone.
#[derive(Copy, Clone)]
pub struct GeoLocation {
    /// The location's latitude and longitude.
    pub coordinates: Coordinates,
    /// The IANA time zone associated with the coordinates.
    pub timezone: Tz,
}

impl fmt::Display for GeoLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.coordinates, self.timezone)
    }
}

impl From<GeoLocation> for sunrise::Coordinates {
    fn from(value: GeoLocation) -> Self {
        value.coordinates.into()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct IpApiResponse {
    latitude: f64,
    longitude: f64,
    timezone: String,
}

/// Fetches an approximate geolocation for the current network connection.
///
/// This uses `https://ipapi.co/json/` to resolve the device's public IP to a
/// latitude/longitude pair and an IANA time zone. The request times out after
/// 10 seconds.
pub fn fetch_geolocation(client: &HttpClient) -> Result<GeoLocation, Error> {
    let resp: IpApiResponse = client
        .get("https://ipapi.co/json/")
        .timeout(Duration::from_secs(10))
        .send()?
        .json()?;

    let timezone = resp
        .timezone
        .parse::<Tz>()
        .map_err(|e| anyhow::anyhow!("invalid timezone from ipapi: {e}"))?;

    let coordinates = Coordinates::new(resp.latitude, resp.longitude)?;

    Ok(GeoLocation {
        coordinates,
        timezone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipapi_response() {
        let json = serde_json::json!({
            "latitude": 51.5074,
            "longitude": -0.1278,
            "timezone": "Europe/London"
        });
        let resp: IpApiResponse = serde_json::from_value(json).unwrap();
        assert!((resp.latitude - 51.5074).abs() < 0.001);
        assert!((resp.longitude - (-0.1278)).abs() < 0.001);
        assert_eq!(resp.timezone, "Europe/London");
    }

    #[test]
    fn coordinates_validate_bounds() {
        assert!(Coordinates::new(51.5074, -0.1278).is_ok());
        assert!(Coordinates::new(-90.0, -180.0).is_ok());
        assert!(Coordinates::new(90.0, 180.0).is_ok());
        assert!(Coordinates::new(f64::NAN, 0.0).is_err());
        assert!(Coordinates::new(0.0, f64::NAN).is_err());
        assert!(Coordinates::new(90.1, 0.0).is_err());
        assert!(Coordinates::new(0.0, 180.1).is_err());
    }
}
