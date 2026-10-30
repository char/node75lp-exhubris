fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    for name in ["board", "keyboard"] {
        let schema = format!("{name}.kdl");
        println!("cargo::rerun-if-changed={schema}");
        let interface = hubris_build::idl::load_interface(&schema).expect("IPC interface");
        let client = hubris_build::idl::codegen::generate_client(&interface).expect("IPC client");
        std::fs::write(
            out.join(format!("{name}.rs")),
            hubris_build::idl::codegen::format_code(&client),
        )
        .expect("write IPC client");
    }
}
