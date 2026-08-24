// Links as a GUI program so a double-clicked JumpPad comes up without a
// console window behind it. The attribute has to live here rather than in
// `lib.rs`: it is read off the crate root of the *binary* being linked, and
// is ignored outright on any other crate type, so a copy in the library did
// nothing (which is worth knowing - one sat there commented out for a while).
//
// Unconditional, not `cfg_attr(not(debug_assertions), ...)`. What brings the
// terminal back is `JUMPPAD_DEBUG=1`, on any build - see `debug.rs`. Tying it
// to the profile instead would mean the console you get while developing is
// not the console you get from a release binary, which is exactly the gap
// that hides a startup bug until someone ships.
#![windows_subsystem = "windows"]

fn main() -> iced::Result {
    jumppad::run()
}
