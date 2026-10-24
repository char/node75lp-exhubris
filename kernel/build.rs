fn main() {
    // The final-image audit inspects relocations in the linked kernel to
    // prove the recovery path is RAM-confined; keep them in the output.
    println!("cargo:rustc-link-arg=--emit-relocs");
    println!("cargo:rustc-link-arg=--nmagic");
    // Without an emulation, LLD labels the kernel GNU because of `#[used]`
    // sections, while hubris-build relinks the tasks as SysV. The bundler
    // refuses to combine mixed OS ABIs.
    println!("cargo:rustc-link-arg=-melf32lriscv");
}
