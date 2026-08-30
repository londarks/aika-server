//! Reads and edits `SL.bin` — the channel list the client shows on its server
//! selection screen.
//!
//! ```text
//! sl-tool list   <SL.bin>
//! sl-tool set-ip <SL.bin> <new-ip> [--out <file>]
//! ```
//!
//! `set-ip` rewrites the IP of every occupied channel and, without `--out`,
//! writes next to the original with a `.new` suffix — never over it, so a
//! mistake cannot ruin the client's file.

use aika_data::sl::ServerList;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match refs.as_slice() {
        ["list", path] => list(path),
        ["set-ip", path, ip] => set_ip(path, ip, &format!("{path}.new")),
        ["set-ip", path, ip, "--out", out] => set_ip(path, ip, out),
        _ => {
            eprintln!("usage:");
            eprintln!("  sl-tool list   <SL.bin>");
            eprintln!("  sl-tool set-ip <SL.bin> <new-ip> [--out <file>]");
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

fn list(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let list = ServerList::decode(&std::fs::read(path)?)?;
    println!("{} slots in the file, occupied:", list.channels.len());
    println!("{:>4}  {:<16} {:<24} {:>6}", "slot", "ip", "name", "nation");
    for (index, channel) in list.occupied() {
        println!(
            "{index:>4}  {:<16} {:<24} {:>6}",
            channel.ip(),
            channel.name(),
            channel.nation_index()
        );
    }
    Ok(())
}

fn set_ip(path: &str, ip: &str, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut list = ServerList::decode(&std::fs::read(path)?)?;

    let mut changed = 0;
    for channel in list.channels.iter_mut().filter(|c| !c.is_empty()) {
        channel.set_ip(ip)?;
        changed += 1;
    }

    std::fs::write(out, list.encode())?;
    println!("{changed} channels now point at {ip}; written to {out}");
    Ok(())
}
