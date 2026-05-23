#[path = "unit/common.rs"]
mod common;

#[path = "unit/lib.rs"]
mod lib_tests;

#[path = "unit/random.rs"]
mod random;

#[path = "unit/ref_resolver.rs"]
mod ref_resolver;

#[path = "unit/schema_walker.rs"]
mod schema_walker;

#[path = "unit/options.rs"]
mod options;

#[path = "unit/extensions.rs"]
mod extensions;

#[path = "unit/formats.rs"]
mod formats;

#[path = "unit/generators/array.rs"]
mod generators_array;

#[path = "unit/generators/object.rs"]
mod generators_object;

#[path = "unit/generators/string.rs"]
mod generators_string;

#[path = "unit/generators/number.rs"]
mod generators_number;

#[path = "unit/generators/boolean.rs"]
mod generators_boolean;

#[path = "unit/generators/null.rs"]
mod generators_null;

#[path = "unit/generators/enum_const.rs"]
mod generators_enum_const;

#[path = "unit/generators/composition.rs"]
mod generators_composition;
