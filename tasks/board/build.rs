fn main() {
    let schema = "../../lib/ipc/board.kdl";
    println!("cargo::rerun-if-changed={schema}");
    let interface = hubris_build::idl::load_interface(schema).expect("board interface");
    let server = hubris_build::idl::codegen::generate_server(&interface).expect("board server");
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        path.join("server.rs"),
        hubris_build::idl::codegen::format_code(&server),
    )
    .expect("write board server");
}
