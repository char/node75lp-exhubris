fn main() {
    println!("cargo::rerun-if-changed=board.kdl");
    let interface = hubris_build::idl::load_interface("board.kdl").expect("board interface");
    let client = hubris_build::idl::codegen::generate_client(&interface).expect("board client");
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        path.join("client.rs"),
        hubris_build::idl::codegen::format_code(&client),
    )
    .expect("write board client");
}
