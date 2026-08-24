// See `jumppad.rs` for why this is here, unconditional, and duplicated
// rather than shared from the library: `windows_subsystem` only counts on
// the crate root of the binary being linked.
#![windows_subsystem = "windows"]

fn main() -> iced::Result {
    jumppad::run()
}
