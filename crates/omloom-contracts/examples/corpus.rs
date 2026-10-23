#[path = "../tests/support/mod.rs"]
mod support;
use omloom_contracts::{Limits, MemoryBudget, decode};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("corpus OUTPUT")?);
    fs::create_dir_all(&root)?;
    let mut manifest = String::new();
    for case in support::cases() {
        fs::write(root.join(format!("{}.bin", case.name)), &case.bytes)?;
        let actual = decode(
            &case.bytes,
            Limits::default(),
            &MemoryBudget::new(16 * 1_048_576),
        )
        .err();
        if actual != case.error {
            return Err(format!("{}: {:?} != {:?}", case.name, actual, case.error).into());
        }
        manifest.push_str(&format!(
            "{}\t{}\n",
            case.name,
            case.error.map_or("ok".into(), |e| e.to_string())
        ));
    }
    fs::write(root.join("manifest.tsv"), manifest)?;
    println!("contract corpus generated and validated");
    Ok(())
}
