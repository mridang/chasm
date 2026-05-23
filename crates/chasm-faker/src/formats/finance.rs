use crate::random::Random;
use fake::faker::creditcard::en::CreditCardNumber;
use fake::faker::currency::en::{CurrencyCode, CurrencyName, CurrencySymbol};
use fake::faker::finance::en::{Bic, Isin};
use fake::Fake;

/// Generates a SWIFT/BIC bank identifier code.
pub fn generate_bic(rng: &mut Random) -> String {
    Bic().fake_with_rng(rng.inner())
}

/// Generates an International Securities Identification Number.
pub fn generate_isin(rng: &mut Random) -> String {
    Isin().fake_with_rng(rng.inner())
}

/// Generates a syntactically valid credit-card number string.
pub fn generate_credit_card_number(rng: &mut Random) -> String {
    CreditCardNumber().fake_with_rng(rng.inner())
}

/// Generates an ISO 4217 currency code like "USD".
pub fn generate_currency_code(rng: &mut Random) -> String {
    CurrencyCode().fake_with_rng(rng.inner())
}

/// Generates a currency name like "US Dollar".
pub fn generate_currency_name(rng: &mut Random) -> String {
    CurrencyName().fake_with_rng(rng.inner())
}

/// Generates a currency symbol character like "$".
pub fn generate_currency_symbol(rng: &mut Random) -> String {
    CurrencySymbol().fake_with_rng(rng.inner())
}
