//! Reads, audits and rewrites `strdef*.bin`, the client's master string table.
//!
//! ```text
//! strdef-tool list         <strdef.bin> [from] [to]
//! strdef-tool pending      <strdef.bin>
//! strdef-tool export       <strdef.bin> <out.tsv>
//! strdef-tool import       <strdef.bin> <in.tsv> [--out <file>]
//! strdef-tool diff         <a.bin> <b.bin>
//! strdef-tool scan         <any file>
//! ```
//!
//! `scan` works on files that are not record tables at all: scene layouts
//! and lore files embed strings inside binary structures, and it walks the
//! bytes looking for the same untranslated text.
//!
//! The working loop is `export`, edit the TSV in any editor, `import`. Records
//! nobody touched come back byte for byte, and `import` never writes over the
//! input unless told to: it saves next to it with a `.new` suffix.
//!
//! `pending` lists the entries still holding Big5 or EUC-KR text, which are the
//! ones the translation never reached.

use aika_data::strdef::{scan_double_byte, StrDef, MAX_TEXT};
use std::process::ExitCode;

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match refs.as_slice() {
        ["list", path] => list(path, 0, usize::MAX),
        ["list", path, from, to] => match (from.parse(), to.parse()) {
            (Ok(from), Ok(to)) => list(path, from, to),
            _ => Err("from and to must be numbers".into()),
        },
        ["pending", path] => pending(path),
        ["export", path, out] => export(path, out),
        ["import", path, tsv] => import(path, tsv, &format!("{path}.new")),
        ["import", path, tsv, "--out", out] => import(path, tsv, out),
        ["diff", a, b] => diff(a, b),
        ["scan", path] => scan(path),
        _ => {
            usage();
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

fn usage() {
    eprintln!("usage:");
    eprintln!("  strdef-tool list    <strdef.bin> [from] [to]");
    eprintln!("  strdef-tool pending <strdef.bin>");
    eprintln!("  strdef-tool export  <strdef.bin> <out.tsv>");
    eprintln!("  strdef-tool import  <strdef.bin> <in.tsv> [--out <file>]");
    eprintln!("  strdef-tool diff    <a.bin> <b.bin>");
    eprintln!("  strdef-tool scan    <any file>");
}

fn load(path: &str) -> Result<StrDef, Box<dyn std::error::Error>> {
    Ok(StrDef::decode(&std::fs::read(path)?)?)
}

fn list(path: &str, from: usize, to: usize) -> Fallible {
    let table = load(path)?;
    println!("{} entries in {path}", table.len());
    for (index, text) in table.occupied() {
        if index >= from && index <= to {
            println!("{index:>5}  {text}");
        }
    }
    Ok(())
}

fn pending(path: &str) -> Fallible {
    let table = load(path)?;
    let mut count = 0;
    for (index, raw) in table.untranslated() {
        let hex: Vec<String> = raw.iter().take(16).map(|b| format!("{b:02X}")).collect();
        println!("{index:>5}  {}", hex.join(" "));
        count += 1;
    }
    if count == 0 {
        println!("no entry left in double-byte text: this table is fully translated");
    } else {
        println!("\n{count} of {} entries still untranslated", table.len());
    }
    Ok(())
}

fn export(path: &str, out: &str) -> Fallible {
    let table = load(path)?;
    let mut text = String::from("# index\ttext\n");
    text.push_str("# edit the right column only; a line may not exceed ");
    text.push_str(&MAX_TEXT.to_string());
    text.push_str(" bytes\n");
    for (index, entry) in table.occupied() {
        text.push_str(&format!("{index}\t{}\n", escape(&entry)));
    }
    std::fs::write(out, text)?;
    println!("{} entries written to {out}", table.occupied().count());
    Ok(())
}

fn import(path: &str, tsv: &str, out: &str) -> Fallible {
    let mut table = load(path)?;
    let text = std::fs::read_to_string(tsv)?;

    let mut changed = 0;
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((index, entry)) = line.split_once('\t') else {
            return Err(format!("line {}: no tab separator", line_no + 1).into());
        };
        let index: usize = index
            .trim()
            .parse()
            .map_err(|_| format!("line {}: '{index}' is not an index", line_no + 1))?;

        let entry = unescape(entry);
        if table.get(index).as_deref() != Some(entry.as_str()) {
            table.set(index, &entry)?;
            changed += 1;
        }
    }

    std::fs::write(out, table.encode())?;
    println!("{changed} entries changed; written to {out}");
    Ok(())
}

fn diff(a: &str, b: &str) -> Fallible {
    let (left, right) = (load(a)?, load(b)?);
    let differences = left.differences(&right);
    println!("{} entries differ between {a} and {b}\n", differences.len());
    for (index, mine, theirs) in &differences {
        println!("{index:>5}  - {}", escape(mine));
        println!("       + {}", escape(theirs));
    }
    Ok(())
}

/// Reports double-byte text anywhere in a file, whatever its structure.
fn scan(path: &str) -> Fallible {
    let bytes = std::fs::read(path)?;
    let found = scan_double_byte(&bytes, 6);

    if found.is_empty() {
        println!("no double-byte text found in {path}");
        return Ok(());
    }

    println!("{} untranslated runs in {path}", found.len());
    for (offset, run) in &found {
        let hex: Vec<String> = run.iter().take(12).map(|b| format!("{b:02X}")).collect();
        println!("  @0x{offset:06X}  {} bytes  {}", run.len(), hex.join(" "));
    }
    Ok(())
}

/// Keeps one entry on one line, since the table format is line based.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
