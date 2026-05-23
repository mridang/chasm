use crate::random::Random;
use fake::faker::company::en::{
    BsAdj, BsNoun, BsVerb, Buzzword, CatchPhrase, CompanyName, CompanySuffix, Industry, Profession,
};
use fake::Fake;

/// Generates a fictional company name.
pub fn generate_company_name(rng: &mut Random) -> String {
    CompanyName().fake_with_rng(rng.inner())
}

/// Generates a company legal suffix like "LLC" or "Inc.".
pub fn generate_company_suffix(rng: &mut Random) -> String {
    CompanySuffix().fake_with_rng(rng.inner())
}

/// Generates an industry name like "Information Technology".
pub fn generate_industry(rng: &mut Random) -> String {
    Industry().fake_with_rng(rng.inner())
}

/// Generates a profession or job title.
pub fn generate_profession(rng: &mut Random) -> String {
    Profession().fake_with_rng(rng.inner())
}

/// Generates a marketing-style buzzword.
pub fn generate_buzzword(rng: &mut Random) -> String {
    Buzzword().fake_with_rng(rng.inner())
}

/// Generates a corporate catch-phrase.
pub fn generate_catch_phrase(rng: &mut Random) -> String {
    CatchPhrase().fake_with_rng(rng.inner())
}

/// Generates a business-speak adjective.
pub fn generate_bs_adjective(rng: &mut Random) -> String {
    BsAdj().fake_with_rng(rng.inner())
}

/// Generates a business-speak noun.
pub fn generate_bs_noun(rng: &mut Random) -> String {
    BsNoun().fake_with_rng(rng.inner())
}

/// Generates a business-speak verb.
pub fn generate_bs_verb(rng: &mut Random) -> String {
    BsVerb().fake_with_rng(rng.inner())
}
