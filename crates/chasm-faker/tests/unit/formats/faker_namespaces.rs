//! Tests asserting that faker-key namespaces are recognised by the walker's allow-list.

use super::assert_faker_namespace_recognized;

/// The `airline.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_airline_recognized() {
    assert_faker_namespace_recognized("airline.airline");
}

/// The `animal.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_animal_recognized() {
    assert_faker_namespace_recognized("animal.dog");
}

/// The `book.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_book_recognized() {
    assert_faker_namespace_recognized("book.title");
}

/// The `food.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_food_recognized() {
    assert_faker_namespace_recognized("food.dish");
}

/// The `location.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_location_recognized() {
    assert_faker_namespace_recognized("location.city");
}

/// The `sport.` faker namespace is recognised by the walker.
#[test]
fn test_faker_namespace_sport_recognized() {
    assert_faker_namespace_recognized("sport.sport");
}
