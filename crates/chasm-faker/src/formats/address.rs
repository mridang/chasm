use crate::random::Random;
use fake::faker::address::en::{
    BuildingNumber, CityName, CountryCode, CountryName, Latitude, Longitude, PostCode,
    SecondaryAddress, StateAbbr, StateName, StreetName, TimeZone, ZipCode,
};
use fake::Fake;

/// Generates a fictional street name.
pub fn generate_street_name(rng: &mut Random) -> String {
    StreetName().fake_with_rng(rng.inner())
}

/// Generates a building number string.
pub fn generate_building_number(rng: &mut Random) -> String {
    BuildingNumber().fake_with_rng(rng.inner())
}

/// Generates a secondary address line like "Apt. 12".
pub fn generate_secondary_address(rng: &mut Random) -> String {
    SecondaryAddress().fake_with_rng(rng.inner())
}

/// Generates a city name.
pub fn generate_city_name(rng: &mut Random) -> String {
    CityName().fake_with_rng(rng.inner())
}

/// Generates a state or province name.
pub fn generate_state_name(rng: &mut Random) -> String {
    StateName().fake_with_rng(rng.inner())
}

/// Generates a state abbreviation like "CA".
pub fn generate_state_abbr(rng: &mut Random) -> String {
    StateAbbr().fake_with_rng(rng.inner())
}

/// Generates a US-style ZIP code string.
pub fn generate_zip_code(rng: &mut Random) -> String {
    ZipCode().fake_with_rng(rng.inner())
}

/// Generates a generic postal code string.
pub fn generate_post_code(rng: &mut Random) -> String {
    PostCode().fake_with_rng(rng.inner())
}

/// Generates a country name.
pub fn generate_country_name(rng: &mut Random) -> String {
    CountryName().fake_with_rng(rng.inner())
}

/// Generates a two-letter country code.
pub fn generate_country_code(rng: &mut Random) -> String {
    CountryCode().fake_with_rng(rng.inner())
}

/// Generates a latitude value as a decimal string.
pub fn generate_latitude(rng: &mut Random) -> String {
    Latitude().fake_with_rng(rng.inner())
}

/// Generates a longitude value as a decimal string.
pub fn generate_longitude(rng: &mut Random) -> String {
    Longitude().fake_with_rng(rng.inner())
}

/// Generates an IANA time zone identifier like "America/New_York".
pub fn generate_time_zone(rng: &mut Random) -> String {
    TimeZone().fake_with_rng(rng.inner())
}
