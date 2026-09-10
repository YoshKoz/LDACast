#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
// bindgen output we do not control: it transmutes where a cast would do and
// does not wrap unsafe calls inside its own unsafe fns.
#![allow(unnecessary_transmutes, unsafe_op_in_unsafe_fn)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
