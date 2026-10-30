#![no_std]
#![no_main]

#[unsafe(export_name = "main")]
fn main() -> ! {
    loop {
        userlib::idle();
    }
}
