use std::path::{Path, PathBuf};

fn glob_c(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("c") {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // canonicalize() yields a \\?\ prefixed path that cl.exe rejects
    let root = manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("third_party/btstack");
    let port = root.join("port/windows-winusb");

    let includes = [
        port.clone(),
        root.join("src"),
        root.join("platform/windows"),
        root.join("platform/embedded"),
        root.join("chipset/zephyr"),
        root.join("3rd-party/bluedroid/decoder/include"),
        root.join("3rd-party/bluedroid/encoder/include"),
        root.join("3rd-party/lc3-google/include"),
        root.join("3rd-party/micro-ecc"),
        root.join("3rd-party/md5"),
        root.join("3rd-party/hxcmod-player"),
        root.join("3rd-party/hxcmod-player/mod"),
        root.join("3rd-party/rijndael"),
        root.join("3rd-party/yxml"),
    ];

    let mut sources = Vec::new();
    for dir in [
        "src",
        "src/classic",
        "src/ble",
        "src/ble/gatt-service",
        "platform/windows",
        "chipset/zephyr",
        "3rd-party/bluedroid/encoder/srce",
        "3rd-party/bluedroid/decoder/srce",
        "3rd-party/hxcmod-player",
        "3rd-party/hxcmod-player/mods",
    ] {
        glob_c(&root.join(dir), &mut sources);
    }
    sources.push(root.join("3rd-party/micro-ecc/uECC.c"));
    sources.push(root.join("3rd-party/md5/md5.c"));
    sources.push(root.join("3rd-party/rijndael/rijndael.c"));
    sources.push(root.join("3rd-party/yxml/yxml.c"));

    // le_device_db_memory.c conflicts with the TLV-backed db used by the port
    sources.retain(|p| p.file_name().unwrap() != "le_device_db_memory.c");

    let mut build = cc::Build::new();
    build.warnings(false);
    for inc in &includes {
        build.include(inc);
    }
    for src in &sources {
        build.file(src);
    }
    build.compile("btstack");

    println!("cargo:rustc-link-lib=winusb");
    println!("cargo:rustc-link-lib=setupapi");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rustc-link-lib=user32");

    let clang_args: Vec<String> = includes
        .iter()
        .map(|i| format!("-I{}", i.display()))
        .collect();

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let extern_c = out.join("extern.c");

    let bindings = bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_args(&clang_args)
        .clang_arg("-DENABLE_CLASSIC")
        .layout_tests(false)
        // btstack's event accessors are static inline; without this they have
        // no symbol to link against
        .wrap_static_fns(true)
        .wrap_static_fns_path(&extern_c)
        // Windows' ANSI/Unicode shims are static inline too and pull in symbols
        // that are not in any import library we need
        .blocklist_function("ua_.*")
        .blocklist_function("uaw_.*")
        // redeclaring the compiler's own mem*/strlen builtins trips
        // suspicious_runtime_symbol_definitions; nothing here calls them
        .blocklist_function("memcpy")
        .blocklist_function("memmove")
        .blocklist_function("memset")
        .blocklist_function("memcmp")
        .blocklist_function("strlen")
        .generate()
        .expect("bindgen failed");

    bindings.write_to_file(out.join("bindings.rs")).unwrap();

    let mut wrappers = cc::Build::new();
    wrappers.warnings(false).include(&manifest);
    for inc in &includes {
        wrappers.include(inc);
    }
    wrappers.file(&extern_c).compile("btstack_static_wrappers");

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
}
