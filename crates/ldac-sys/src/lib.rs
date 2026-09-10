#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(test)]
mod tests {
    #[test]
    fn encoder_links_and_reports_version() {
        let v = unsafe { super::ldacBT_get_version() };
        assert!(v > 0, "ldacBT_get_version returned {v}");
    }

    #[test]
    fn handle_alloc_roundtrip() {
        unsafe {
            let h = super::ldacBT_get_handle();
            assert!(!h.is_null());
            super::ldacBT_free_handle(h);
        }
    }
}
