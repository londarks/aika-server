//! Converts the client's `.jit` textures to DDS and back, so the interface can
//! be edited in any image tool.
//!
//! ```text
//! jit-tool info    <file.jit>
//! jit-tool to-dds  <file.jit> [--out <file.dds>]
//! jit-tool to-jit  <original.jit> <edited.dds> [--out <file.jit>]
//! ```
//!
//! The workflow is `to-dds`, edit the DDS in Photoshop, GIMP or Paint.NET,
//! then `to-jit`. Pixel data is copied untouched in both directions, so a
//! round trip changes nothing.
//!
//! `to-jit` takes the original file as a template on purpose: it carries the
//! magic tag we do not fully interpret, and it lets the tool refuse a
//! replacement whose size or compression the client would not accept. Output
//! goes to a `.new` file rather than over the original.

use aika_data::jit::{Jit, DDS_HEADER_SIZE};
use std::process::ExitCode;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match refs.as_slice() {
        ["info", path] => info(path),
        ["to-dds", path] => to_dds(path, &swap_extension(path, "dds")),
        ["to-dds", path, "--out", out] => to_dds(path, out),
        ["to-jit", original, dds] => to_jit(original, dds, &format!("{original}.new")),
        ["to-jit", original, dds, "--out", out] => to_jit(original, dds, out),
        _ => {
            eprintln!("usage:");
            eprintln!("  jit-tool info   <file.jit>");
            eprintln!("  jit-tool to-dds <file.jit> [--out <file.dds>]");
            eprintln!("  jit-tool to-jit <original.jit> <edited.dds> [--out <file.jit>]");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn swap_extension(path: &str, extension: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.{extension}"),
        None => format!("{path}.{extension}"),
    }
}

fn info(path: &str) -> Fallible {
    let texture = Jit::decode(&std::fs::read(path)?)?;
    println!("{path}");
    println!("  magic       {}", String::from_utf8_lossy(&texture.magic));
    println!("  size        {} x {}", texture.width, texture.height);
    println!("  compression {:?}", texture.format);
    println!("  mip levels  {}", texture.levels);
    println!("  payload     {} bytes", texture.data.len());
    Ok(())
}

fn to_dds(path: &str, out: &str) -> Fallible {
    let texture = Jit::decode(&std::fs::read(path)?)?;
    let dds = texture.to_dds();
    std::fs::write(out, &dds)?;
    println!(
        "{} x {} {:?}, {} mip level(s) written to {out}",
        texture.width, texture.height, texture.format, texture.levels
    );
    Ok(())
}

fn to_jit(original: &str, dds: &str, out: &str) -> Fallible {
    let template = Jit::decode(&std::fs::read(original)?)?;
    let edited = std::fs::read(dds)?;
    let rebuilt = template.replace_from_dds(&edited)?;

    std::fs::write(out, rebuilt.encode())?;
    println!(
        "{} x {} {:?} written to {out} ({} bytes of pixels)",
        rebuilt.width,
        rebuilt.height,
        rebuilt.format,
        edited.len() - DDS_HEADER_SIZE
    );
    Ok(())
}
