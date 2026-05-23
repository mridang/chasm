pub mod address;
pub mod barcode;
pub mod boolean;
pub mod color;
pub mod company;
pub mod date_time;
pub mod duration;
pub mod email;
pub mod filesystem;
pub mod finance;
pub mod hostname;
pub mod http;
pub mod idn;
pub mod internet_extras;
pub mod ip;
pub mod json_pointer;
pub mod lorem;
pub mod name;
pub mod path_aliases;
pub mod phone;
pub mod pydantic_extras;
pub mod pydantic_numeric;
pub mod uri;
pub mod uri_template;
pub mod uuid;

use crate::options::GenerateOptions;
use crate::random::Random;
use serde_json::Value;

/// Returns true when `format` is one of the format names the dispatcher recognises.
///
/// Used by the string generator to decide between strict-error vs lenient-fallback
/// handling when `fail_on_invalid_format` is enabled.
pub fn is_known_format(format: &str) -> bool {
    matches!(
        format,
        "email"
            | "idn-email"
            | "uuid"
            | "uuid1"
            | "uuid3"
            | "uuid4"
            | "uuid5"
            | "guid"
            | "date-time"
            | "datetime"
            | "iso-date-time"
            | "date"
            | "iso-date"
            | "full-date"
            | "time"
            | "partial-time"
            | "uri"
            | "url"
            | "iri"
            | "uri-reference"
            | "iri-reference"
            | "hostname"
            | "idn-hostname"
            | "ipv4"
            | "ipv6"
            | "json-pointer"
            | "relative-json-pointer"
            | "duration"
            | "password"
            | "byte"
            | "base64url"
            | "binary"
            | "regex"
            | "decimal"
            | "float"
            | "double"
            | "int32"
            | "int64"
            | "slug"
            | "phone"
            | "color"
            | "colour"
            | "hex-color"
            | "name"
            | "full-name"
            | "first-name"
            | "given-name"
            | "last-name"
            | "family-name"
            | "surname"
            | "name-with-title"
            | "title"
            | "suffix"
            | "company"
            | "company-name"
            | "company-suffix"
            | "industry"
            | "profession"
            | "job-title"
            | "job"
            | "catch-phrase"
            | "catchphrase"
            | "buzzword"
            | "bs-adjective"
            | "bs-noun"
            | "bs-verb"
            | "street-name"
            | "street"
            | "building-number"
            | "secondary-address"
            | "city"
            | "city-name"
            | "state"
            | "state-name"
            | "state-abbr"
            | "state-code"
            | "zip"
            | "zip-code"
            | "postcode"
            | "post-code"
            | "country"
            | "country-name"
            | "country-code"
            | "latitude"
            | "lat"
            | "longitude"
            | "lng"
            | "long"
            | "lon"
            | "timezone"
            | "time-zone"
            | "phone-number"
            | "phonenumber"
            | "username"
            | "user-name"
            | "mac"
            | "mac-address"
            | "macaddress"
            | "user-agent"
            | "useragent"
            | "userAgent"
            | "domain"
            | "domain-name"
            | "hostname-suffix"
            | "free-email"
            | "free-email-provider"
            | "lorem-word"
            | "word"
            | "lorem-words"
            | "words"
            | "lorem-sentence"
            | "sentence"
            | "lorem-sentences"
            | "sentences"
            | "lorem-paragraph"
            | "paragraph"
            | "lorem-paragraphs"
            | "paragraphs"
            | "bic"
            | "swift"
            | "isin"
            | "credit-card"
            | "creditcard"
            | "credit-card-number"
            | "currency"
            | "currency-code"
            | "currency-name"
            | "currency-symbol"
            | "file-name"
            | "filename"
            | "file-extension"
            | "extension"
            | "file-path"
            | "filepath"
            | "path"
            | "mime"
            | "mime-type"
            | "mimetype"
            | "semver"
            | "semantic-version"
            | "version"
            | "semver-stable"
            | "semver-unstable"
            | "boolean"
            | "bool"
            | "rgb"
            | "rgb-color"
            | "rgba"
            | "rgba-color"
            | "hsl"
            | "hsl-color"
            | "hsla"
            | "hsla-color"
            | "http-status"
            | "status-code"
            | "rfc-status-code"
            | "valid-status-code"
            | "isbn"
            | "isbn10"
            | "isbn-10"
            | "isbn13"
            | "isbn-13"
            | "secret-str"
            | "secret-bytes"
            | "directory-path"
            | "new-path"
            | "negative-int"
            | "positive-int"
            | "nonnegative-int"
            | "nonpositive-int"
            | "strict-int"
            | "strict-bool"
            | "strict-float"
            | "strict-str"
            | "uri-template"
            | "condate"
            | "condecimal"
            | "aware-datetime"
            | "naive-datetime"
            | "aware-date"
            | "naive-date"
            | "name-email"
            | "json-string"
    )
}

/// Dispatches a format value with both an optional schema node and optional generator options.
///
/// The `opts` parameter currently feeds `minDateTime`/`maxDateTime` bounds into the
/// `date-time` generator; other formats ignore it. Provided as a separate entry point
/// so callers that have no options struct (e.g. extension shims) keep working unchanged.
pub fn generate_for_format_with_options(
    format: &str,
    rng: &mut Random,
    schema: Option<&Value>,
    opts: Option<&GenerateOptions>,
) -> String {
    if let Some(Value::String(s)) = crate::invoke_format(format, rng) {
        return s;
    }
    match format {
        "email" => email::generate(rng),
        "idn-email" => idn::generate_idn_email(rng),
        "uuid" | "uuid1" | "uuid3" | "uuid4" | "uuid5" | "guid" => uuid::generate(rng),
        "date-time" | "datetime" | "iso-date-time" => {
            let min = opts.and_then(|o| o.min_date_time.as_deref());
            let max = opts.and_then(|o| o.max_date_time.as_deref());
            if min.is_some() || max.is_some() {
                return date_time::generate_datetime_with_bounds(rng, min, max);
            }
            date_time::generate_datetime(rng)
        }
        "date" | "iso-date" | "full-date" => date_time::generate_date(rng),
        "time" | "partial-time" => date_time::generate_time(rng),
        "uri" | "url" | "uri-reference" | "iri-reference" => uri::generate(rng),
        "iri" => idn::generate_iri(rng),
        "hostname" => hostname::generate(rng),
        "idn-hostname" => idn::generate_idn_hostname(rng),
        "ipv4" => ip::generate_ipv4(rng),
        "ipv6" => ip::generate_ipv6(rng),
        "json-pointer" => json_pointer::generate(rng),
        "relative-json-pointer" => {
            let n = rng.int(0, 5);
            let ptr = json_pointer::generate(rng);
            format!("{}{}", n, ptr)
        }
        "duration" => duration::generate(rng),
        "password" => generate_password(rng),
        "byte" => generate_byte(rng),
        "base64url" => generate_base64_url(rng),
        "binary" => generate_binary(rng),
        "regex" => generate_regex_from_schema(rng, schema),
        "decimal" => generate_decimal(rng),
        "float" | "double" => generate_float_string(rng),
        "int32" => generate_int32_string(rng),
        "int64" => generate_int64_string(rng),
        "slug" => generate_slug(rng),
        "phone" => generate_phone(rng),
        "hex-color" => generate_hex_color(rng),
        "name" | "full-name" => name::generate_full_name(rng),
        "first-name" | "given-name" => name::generate_first_name(rng),
        "last-name" | "family-name" | "surname" => name::generate_last_name(rng),
        "name-with-title" => name::generate_name_with_title(rng),
        "title" => name::generate_title(rng),
        "suffix" => name::generate_suffix(rng),
        "company" | "company-name" => company::generate_company_name(rng),
        "company-suffix" => company::generate_company_suffix(rng),
        "industry" => company::generate_industry(rng),
        "profession" | "job-title" | "job" => company::generate_profession(rng),
        "catch-phrase" | "catchphrase" => company::generate_catch_phrase(rng),
        "buzzword" => company::generate_buzzword(rng),
        "bs-adjective" => company::generate_bs_adjective(rng),
        "bs-noun" => company::generate_bs_noun(rng),
        "bs-verb" => company::generate_bs_verb(rng),
        "street-name" | "street" => address::generate_street_name(rng),
        "building-number" => address::generate_building_number(rng),
        "secondary-address" => address::generate_secondary_address(rng),
        "city" | "city-name" => address::generate_city_name(rng),
        "state" | "state-name" => address::generate_state_name(rng),
        "state-abbr" | "state-code" => address::generate_state_abbr(rng),
        "zip" | "zip-code" => address::generate_zip_code(rng),
        "postcode" | "post-code" => address::generate_post_code(rng),
        "country" | "country-name" => address::generate_country_name(rng),
        "country-code" => address::generate_country_code(rng),
        "latitude" | "lat" => address::generate_latitude(rng),
        "longitude" | "lng" | "long" | "lon" => address::generate_longitude(rng),
        "timezone" | "time-zone" => address::generate_time_zone(rng),
        "phone-number" | "phonenumber" => phone::generate_phone_number(rng),
        "username" | "user-name" => internet_extras::generate_username(rng),
        "mac" | "mac-address" | "macaddress" => internet_extras::generate_mac_address(rng),
        "user-agent" | "useragent" | "userAgent" => internet_extras::generate_user_agent(rng),
        "domain" | "domain-name" | "hostname-suffix" => {
            internet_extras::generate_domain_suffix(rng)
        }
        "free-email" => internet_extras::generate_free_email(rng),
        "free-email-provider" => internet_extras::generate_free_email_provider(rng),
        "lorem-word" | "word" => lorem::generate_word(rng),
        "lorem-words" | "words" => lorem::generate_words(rng),
        "lorem-sentence" | "sentence" => lorem::generate_sentence(rng),
        "lorem-sentences" | "sentences" => lorem::generate_sentences(rng),
        "lorem-paragraph" | "paragraph" => lorem::generate_paragraph(rng),
        "lorem-paragraphs" | "paragraphs" => lorem::generate_paragraphs(rng),
        "bic" | "swift" => finance::generate_bic(rng),
        "isin" => finance::generate_isin(rng),
        "credit-card" | "creditcard" | "credit-card-number" => {
            finance::generate_credit_card_number(rng)
        }
        "currency" | "currency-code" => finance::generate_currency_code(rng),
        "currency-name" => finance::generate_currency_name(rng),
        "currency-symbol" => finance::generate_currency_symbol(rng),
        "file-name" | "filename" => filesystem::generate_file_name(rng),
        "file-extension" | "extension" => filesystem::generate_file_extension(rng),
        "file-path" | "filepath" | "path" => filesystem::generate_file_path(rng),
        "mime" | "mime-type" | "mimetype" => filesystem::generate_mime_type(rng),
        "semver" | "semantic-version" | "version" => filesystem::generate_semver(rng),
        "semver-stable" => filesystem::generate_semver_stable(rng),
        "semver-unstable" => filesystem::generate_semver_unstable(rng),
        "boolean" | "bool" => boolean::generate_boolean(rng).to_string(),
        "rgb" | "rgb-color" => color::generate_rgb_color(rng),
        "rgba" | "rgba-color" => color::generate_rgba_color(rng),
        "hsl" | "hsl-color" => color::generate_hsl_color(rng),
        "hsla" | "hsla-color" => color::generate_hsla_color(rng),
        "color" | "colour" => color::generate_color(rng),
        "http-status" | "status-code" | "rfc-status-code" => http::generate_rfc_status_code(rng),
        "valid-status-code" => http::generate_valid_status_code(rng),
        "isbn" => barcode::generate_isbn(rng),
        "isbn10" | "isbn-10" => barcode::generate_isbn10(rng),
        "isbn13" | "isbn-13" => barcode::generate_isbn13(rng),
        "secret-str" => generate_password(rng),
        "secret-bytes" => generate_byte(rng),
        "directory-path" => path_aliases::generate_directory_path(rng),
        "new-path" => path_aliases::generate_new_path(rng),
        "negative-int" => pydantic_numeric::generate_negative_int(rng),
        "positive-int" => pydantic_numeric::generate_positive_int(rng),
        "nonnegative-int" => pydantic_numeric::generate_nonnegative_int(rng),
        "nonpositive-int" => pydantic_numeric::generate_nonpositive_int(rng),
        "strict-int" => pydantic_numeric::generate_strict_int(rng),
        "strict-bool" => pydantic_numeric::generate_strict_bool(rng),
        "strict-float" => pydantic_numeric::generate_strict_float(rng),
        "strict-str" => generate_short_word(rng),
        "uri-template" => uri_template::generate_uri_template(rng),
        "condate" | "aware-date" | "naive-date" => pydantic_extras::generate_condate(rng),
        "condecimal" => pydantic_extras::generate_condecimal(rng),
        "aware-datetime" => pydantic_extras::generate_aware_datetime(rng),
        "naive-datetime" => pydantic_extras::generate_naive_datetime(rng),
        "name-email" => pydantic_extras::generate_name_email(rng),
        "json-string" => pydantic_extras::generate_json_string(rng),
        _ => generate_short_word(rng),
    }
}

/// Generates a random 12-character alphanumeric password string.
fn generate_password(rng: &mut Random) -> String {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$"
        .chars()
        .collect();
    (0..12).map(|_| *rng.pick_char(&chars)).collect()
}

/// Generates a base64-encoded string with correct padding for a varying input length.
///
/// Picks an input byte length in `[6, 24]` and computes the standard base64 padding
/// pattern: zero `=` for input lengths that are a multiple of three, one `=` for
/// lengths with remainder two, two `==` for lengths with remainder one. The output
/// alphabet is the standard base64 alphabet (`+` and `/`). This matches RFC 4648 §4
/// padding semantics so consumers that parse the value see realistic shape variety
/// rather than the previous always-`==` suffix.
fn generate_byte(rng: &mut Random) -> String {
    let alphabet: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .chars()
        .collect();
    // Input byte length determines padding: 3n → no `=`, 3n+1 → `==`, 3n+2 → `=`.
    let input_len = rng.int(6, 24) as usize;
    let full_groups = input_len / 3;
    let remainder = input_len % 3;
    let base_chars = full_groups * 4;
    let (extra_chars, padding) = match remainder {
        0 => (0_usize, ""),
        1 => (2_usize, "=="),
        2 => (3_usize, "="),
        _ => unreachable!(),
    };
    let total_chars = base_chars + extra_chars;
    let s: String = (0..total_chars)
        .map(|_| *rng.pick_char(&alphabet))
        .collect();
    format!("{}{}", s, padding)
}

/// Generates an unpadded base64url-encoded string of 8-16 random characters.
///
/// Uses the URL-safe alphabet (`-` and `_` in place of `+` and `/`) and emits no
/// `=` padding, matching the encoding required by JWT compact serialisation and
/// the `format: base64url` JSON Schema convention.
fn generate_base64_url(rng: &mut Random) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let len = rng.int(8, 16) as usize;
    (0..len)
        .map(|_| alphabet[rng.byte() as usize % 64] as char)
        .collect()
}

/// Generates a random hexadecimal string representing binary data.
fn generate_binary(rng: &mut Random) -> String {
    let hex: Vec<char> = "0123456789abcdef".chars().collect();
    let len = rng.int(4, 16) as usize;
    (0..len * 2).map(|_| *rng.pick_char(&hex)).collect()
}

/// Generates a simple valid regex pattern string.
fn generate_regex_string(rng: &mut Random) -> String {
    let patterns = ["[a-z]+", "[0-9]+", "\\w+", ".+", "[a-zA-Z0-9]+"];
    let idx = rng.pick_index(patterns.len());
    patterns[idx].to_string()
}

/// Returns the schema's own `pattern` value when present, otherwise a generic regex literal.
///
/// The `regex` format expects the value to itself be a valid regular expression. When the
/// schema declares both `format: regex` and a sibling `pattern`, returning the pattern is
/// usually the most useful behaviour.
fn generate_regex_from_schema(rng: &mut Random, schema: Option<&Value>) -> String {
    if let Some(Value::Object(map)) = schema {
        if let Some(Value::String(p)) = map.get("pattern") {
            return p.clone();
        }
    }
    generate_regex_string(rng)
}

/// Generates a decimal-shaped string with two fractional digits, e.g. `123.45`.
fn generate_decimal(rng: &mut Random) -> String {
    let whole = rng.int(0, 9999);
    let frac = rng.int(0, 99);
    format!("{}.{:02}", whole, frac)
}

/// Generates a string representation of a random floating-point number.
fn generate_float_string(rng: &mut Random) -> String {
    let v = rng.float(-1000.0, 1000.0);
    format!("{:.4}", v)
}

/// Generates a string representation of a random i32-bounded integer.
fn generate_int32_string(rng: &mut Random) -> String {
    let v = rng.int(i32::MIN as i64, i32::MAX as i64);
    v.to_string()
}

/// Generates a string representation of a random i64 integer.
fn generate_int64_string(rng: &mut Random) -> String {
    let v = rng.int(-1_000_000_000, 1_000_000_000);
    v.to_string()
}

/// Generates a lowercase hyphen-separated slug, e.g. `quick-brown-fox`.
fn generate_slug(rng: &mut Random) -> String {
    let word_chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    let word_count = rng.int(2, 4) as usize;
    let mut parts: Vec<String> = Vec::with_capacity(word_count);
    for _ in 0..word_count {
        let len = rng.int(3, 8) as usize;
        let w: String = (0..len).map(|_| *rng.pick_char(&word_chars)).collect();
        parts.push(w);
    }
    parts.join("-")
}

/// Generates a placeholder phone number string in `+1-555-1234567` shape.
fn generate_phone(rng: &mut Random) -> String {
    let digits: Vec<char> = "0123456789".chars().collect();
    let area: String = (0..3).map(|_| *rng.pick_char(&digits)).collect();
    let num: String = (0..7).map(|_| *rng.pick_char(&digits)).collect();
    format!("+1-{}-{}", area, num)
}

/// Generates an RGB hex color string, e.g. `#A1B2C3`.
fn generate_hex_color(rng: &mut Random) -> String {
    let hex: Vec<char> = "0123456789ABCDEF".chars().collect();
    let s: String = (0..6).map(|_| *rng.pick_char(&hex)).collect();
    format!("#{}", s)
}

/// Generates a short random lowercase alphabetic word for unknown formats.
pub fn generate_short_word(rng: &mut Random) -> String {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
    let len = rng.int(4, 10) as usize;
    (0..len).map(|_| *rng.pick_char(&chars)).collect()
}
