//! Builds JumpPad's icon font from the drawings in `assets/icons`.
//!
//! Run it with `cargo build_fonts`. It walks [`jumppad_icons::ICONS`], turns
//! each named drawing into a glyph and writes the font the app embeds, so
//! the manifest the UI reads and the font it draws from cannot disagree
//! about what is in the font.
//!
//! The output is committed alongside the drawings, which is why the build is
//! deterministic: the same drawings produce the same bytes, and rebuilding
//! without changing an icon leaves the working tree clean.

mod font_file;
mod icon_outline;

use jumppad_icons::Icon;
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

const DRAWINGS: &str = "assets/icons";
const FONT: &str = "assets/fonts/jumppad-icons.ttf";

fn main() -> ExitCode {
    match build() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("build_fonts: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build() -> Result<(), Box<dyn Error>> {
    let root = workspace_root();
    let glyphs = jumppad_icons::ICONS
        .iter()
        .map(|icon| read_glyph(&root, icon))
        .collect::<Result<Vec<_>, _>>()?;
    let font = font_file::assemble(&glyphs)?;

    let destination = root.join(FONT);
    fs::create_dir_all(destination.parent().expect("font path has a parent"))?;
    fs::write(&destination, &font)?;
    report(&destination, font.len(), &glyphs);
    Ok(())
}

fn read_glyph(
    root: &Path,
    icon: &Icon,
) -> Result<font_file::Glyph, Box<dyn Error>> {
    let file = root.join(DRAWINGS).join(format!("{}.svg", icon.drawing));
    let drawing = fs::read_to_string(&file)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    let outline = icon_outline::outline(&drawing)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(font_file::Glyph {
        name: icon.name,
        codepoint: icon.codepoint,
        outline,
    })
}

fn report(destination: &Path, size: usize, glyphs: &[font_file::Glyph]) {
    println!("{} ({size} bytes)", destination.display());
    for glyph in glyphs {
        println!("  {:<12} U+{:04X}", glyph.name, glyph.codepoint as u32);
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two directories under the workspace root")
        .to_path_buf()
}
