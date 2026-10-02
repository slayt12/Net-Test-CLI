//! Developer tasks for nettest. Not part of the shipped binaries.
//!
//! `cargo run -p xtask -- icons` renders `assets/icon.svg` to PNGs (16..512 px) and a
//! multi-size `assets/icon.ico` for the Windows executables. Uses resvg (pure Rust) so no
//! system rasteriser is needed. The ICO stores PNG-compressed images, which Windows has
//! accepted since Vista and keeps the file small.
//!
//! Exit codes: 0 ok, 1 usage, 2 render/IO failure.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use resvg::tiny_skia;
use resvg::usvg;

const SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512];
const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icons") => match icons() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: cargo run -p xtask -- icons");
            ExitCode::from(1)
        }
    }
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/xtask at compile time.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repo root")
        .to_path_buf()
}

fn icons() -> Result<(), String> {
    let assets = repo_root().join("assets");
    let svg_path = assets.join("icon.svg");
    let svg = std::fs::read(&svg_path).map_err(|e| format!("read {}: {e}", svg_path.display()))?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
        .map_err(|e| format!("parse {}: {e}", svg_path.display()))?;

    let mut pngs: Vec<(u32, Vec<u8>)> = Vec::new();
    for &size in SIZES {
        let png = render_png(&tree, size)?;
        let out = assets.join(format!("icon-{size}.png"));
        std::fs::write(&out, &png).map_err(|e| format!("write {}: {e}", out.display()))?;
        println!("wrote {} ({} bytes)", out.display(), png.len());
        pngs.push((size, png));
    }
    // Convenience copy at the canonical name used by README / packaging.
    let canonical = assets.join("icon.png");
    std::fs::copy(assets.join("icon-512.png"), &canonical)
        .map_err(|e| format!("copy icon.png: {e}"))?;

    let ico = build_ico(pngs.iter().filter(|(s, _)| ICO_SIZES.contains(s)));
    let ico_path = assets.join("icon.ico");
    std::fs::write(&ico_path, &ico).map_err(|e| format!("write {}: {e}", ico_path.display()))?;
    println!(
        "wrote {} ({} bytes, {} images)",
        ico_path.display(),
        ico.len(),
        ICO_SIZES.len()
    );
    Ok(())
}

fn render_png(tree: &usvg::Tree, size: u32) -> Result<Vec<u8>, String> {
    let mut pixmap =
        tiny_skia::Pixmap::new(size, size).ok_or_else(|| format!("pixmap {size}px"))?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|e| format!("png encode {size}px: {e}"))
}

/// ICONDIR + ICONDIRENTRY[] + image data. Little-endian throughout.
fn build_ico<'a>(images: impl Iterator<Item = &'a (u32, Vec<u8>)>) -> Vec<u8> {
    let images: Vec<&(u32, Vec<u8>)> = images.collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        // 256 is encoded as 0 in the one-byte width/height fields.
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0); // palette colours
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        out.extend_from_slice(png);
    }
    out
}
