use omloom_contracts::{Limits, MemoryBudget, decode};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("verify CORPUS")?);
    let manifest = fs::read_to_string(root.join("manifest.tsv"))?;
    let mut count = 0;
    for line in manifest.lines() {
        let (name, expected) = line.split_once('\t').ok_or("invalid manifest")?;
        if expected != "ok" {
            continue;
        }
        let original = fs::read(root.join(format!("{name}.bin")))?;
        let encoded = fs::read(root.join(format!("{name}.ts.bin")))?;
        let budget = MemoryBudget::new(16 * 1_048_576);
        let before = decode(&original, Limits::default(), &budget)?;
        let after = decode(&encoded, Limits::default(), &budget)?;
        if before.wire() != after.wire() {
            return Err(format!("lost fields: {name}").into());
        }
        count += 1;
    }
    if count != 8 {
        return Err("missing interoperability record kinds".into());
    }
    println!("{count} TypeScript-produced records admitted unchanged by Rust");
    Ok(())
}
