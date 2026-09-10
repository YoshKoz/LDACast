use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // canonicalize() yields a \\?\ prefixed path that cl.exe rejects
    let root = manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("third_party/ldacBT");
    let inc = root.join("libldac/inc");
    let abr_inc = root.join("libldac/abr/inc");
    let compat = root.join("compat");

    let mut build = cc::Build::new();
    build
        .warnings(false)
        .include(&inc)
        .include(&abr_inc)
        .include(&compat)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        // libldac is a unity build: ldaclib.c and ldacBT.c pull in the rest
        .file(root.join("libldac/src/ldaclib.c"))
        .file(root.join("libldac/src/ldacBT.c"))
        .file(root.join("libldac/abr/src/ldacBT_abr.c"));
    if build.get_compiler().is_like_msvc() {
        build.flag("/FImsvc_compat.h");
    }
    build.compile("ldacBT");

    let bindings = bindgen::Builder::default()
        .header(inc.join("ldacBT.h").to_string_lossy())
        .header(abr_inc.join("ldacBT_abr.h").to_string_lossy())
        .clang_arg(format!("-I{}", inc.display()))
        .clang_arg(format!("-I{}", abr_inc.display()))
        .allowlist_function("ldacBT.*")
        .allowlist_type("LDACBT.*|HANDLE_LDAC.*")
        .allowlist_var("LDACBT.*")
        .layout_tests(false)
        .generate()
        .expect("bindgen failed");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("bindings.rs")).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
}
