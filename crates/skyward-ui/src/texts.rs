//! What the pages say, in every language they speak: each language's file
//! in `locales`, as the build script writes it out. A table there is a
//! module here; a text that names no value is a constant holding it in
//! every language, for [`Lang::pick`](crate::lang::Lang::pick) to choose
//! from, and one that names values in braces is a function that writes it
//! with them.

// Named partition backup choices supersede the upstream grouped rows;
// keep their translations so future upstream catalog updates merge cleanly.
#![expect(dead_code, reason = "the catalog retains descriptions of the upstream grouped backup rows")]

include!(concat!(env!("OUT_DIR"), "/texts.rs"));
