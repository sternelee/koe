use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    generate_shared_buffer_contract(&crate_dir);

    cbindgen::Builder::new()
        .with_crate(crate_dir.to_str().unwrap())
        .with_config(cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")).unwrap())
        .with_language(cbindgen::Language::C)
        .generate()
        .expect("Unable to generate C bindings")
        .write_to_file(crate_dir.join("target/koe_core.h"));
}

fn generate_shared_buffer_contract(crate_dir: &Path) {
    let header_path = crate_dir.join("../KoeApp/VirtualMicPlugin/Sources/shared_buffer_protocol.h");
    println!("cargo:rerun-if-changed={}", header_path.display());

    let header =
        fs::read_to_string(&header_path).expect("Unable to read shared buffer protocol header");
    let shared_buffer_file_path = parse_c_string_define(&header, "KOE_SHARED_BUFFER_FILE_PATH")
        .expect("KOE_SHARED_BUFFER_FILE_PATH missing from shared_buffer_protocol.h");

    let generated = format!(
        "pub const SHARED_BUFFER_FILE_PATH: &str = {:?};\n",
        shared_buffer_file_path
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("shared_buffer_contract.rs"), generated)
        .expect("Unable to write shared buffer contract");
}

fn parse_c_string_define(contents: &str, name: &str) -> Option<String> {
    let prefix = format!("#define {name} ");
    for line in contents.lines() {
        if let Some(value) = line.trim().strip_prefix(&prefix) {
            let value = value.trim();
            if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                return Some(value[1..value.len() - 1].to_string());
            }
        }
    }
    None
}
