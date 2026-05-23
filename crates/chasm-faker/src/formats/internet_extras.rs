use crate::random::Random;
use fake::faker::internet::en::{
    DomainSuffix, FreeEmail, FreeEmailProvider, MACAddress, UserAgent, Username,
};
use fake::Fake;

/// Generates an internet-style username.
pub fn generate_username(rng: &mut Random) -> String {
    Username().fake_with_rng(rng.inner())
}

/// Generates a MAC address string.
pub fn generate_mac_address(rng: &mut Random) -> String {
    MACAddress().fake_with_rng(rng.inner())
}

/// Generates a browser User-Agent string.
pub fn generate_user_agent(rng: &mut Random) -> String {
    UserAgent().fake_with_rng(rng.inner())
}

/// Generates a domain suffix like "com" or "org".
pub fn generate_domain_suffix(rng: &mut Random) -> String {
    DomainSuffix().fake_with_rng(rng.inner())
}

/// Generates a free webmail email address.
pub fn generate_free_email(rng: &mut Random) -> String {
    FreeEmail().fake_with_rng(rng.inner())
}

/// Generates a free webmail provider domain.
pub fn generate_free_email_provider(rng: &mut Random) -> String {
    FreeEmailProvider().fake_with_rng(rng.inner())
}
