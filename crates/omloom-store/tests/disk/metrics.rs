use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
struct DiskState {
    logical: u64,
    allocated: Option<u64>,
    files: BTreeMap<PathBuf, [u8; 32]>,
}

fn disk_state(root: &Path) -> Result<DiskState, Box<dyn std::error::Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    let mut logical = 0u64;
    #[cfg(unix)]
    let mut allocated = 0u64;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            assert!(kind.is_file(), "fixture must not contain links");
            let metadata = entry.metadata()?;
            logical += metadata.len();
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                allocated += metadata.blocks() * 512;
            }
            let mut file = std::fs::File::open(entry.path())?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 8192];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            files.insert(
                entry.path().strip_prefix(root)?.to_path_buf(),
                hash.finalize().into(),
            );
        }
    }
    #[cfg(unix)]
    let allocated = Some(allocated);
    #[cfg(not(unix))]
    let allocated = None;
    Ok(DiskState {
        logical,
        allocated,
        files,
    })
}

// Observe actual writes and journal bytes, independently of asynchronous SST maintenance.
#[test]
fn reads_and_exact_replay_do_not_write_partitions_or_journal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    let record = admit(record::Body::Asset(asset.clone()));
    store.save_asset(&record, None, "admin", 1)?;
    store.diagnostic_database().persist()?;
    let mut taps = Vec::new();
    for partition in coordinode_storage::engine::partition::Partition::all() {
        let (tap, _) = store
            .diagnostic_database()
            .engine()
            .tap_writes(*partition)?;
        taps.push(tap);
    }
    let before = disk_state(directory.path())?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    let usage = store.usage()?;
    let mut times = Vec::with_capacity(200);
    for _ in 0..200 {
        let start = std::time::Instant::now();
        store.asset(target)?;
        times.push(start.elapsed());
        store.save_asset(&record, Some(1), "admin", 2)?;
    }
    let after_reads = disk_state(directory.path())?;
    for tap in &taps {
        assert_eq!(
            tap.take(),
            coordinode_storage::engine::tap::Tapped::Keys(Vec::new()),
            "unexpected {:?} write",
            tap.partition()
        );
    }
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    let journal = |state: &DiskState| {
        state
            .files
            .iter()
            .filter(|(path, _)| path.starts_with("oplog"))
            .map(|(path, hash)| (path.clone(), *hash))
            .collect::<BTreeMap<_, _>>()
    };
    assert!(
        !journal(&before).is_empty(),
        "the fixture must use a real journal"
    );
    assert_eq!(journal(&after_reads), journal(&before));
    assert_eq!(store.usage()?, usage);
    times.sort_unstable();
    eprintln!(
        "read diagnostics (development build): p50={:?}, p95={:?}, p99={:?}; logical={} -> {} bytes, allocated={:?} -> {:?} bytes; partition writes=0, journal changes=0, journal syncs=0; physical files unchanged={}",
        times[100],
        times[190],
        times[198],
        before.logical,
        after_reads.logical,
        before.allocated,
        after_reads.allocated,
        before == after_reads
    );
    let mut edited = asset;
    edited.intended[0].field = Some(text_field("name", "meaningful-change"));
    store.save_asset(&admit(record::Body::Asset(edited)), Some(1), "admin", 3)?;
    assert!(
        taps.iter()
            .any(|tap| tap.take() != coordinode_storage::engine::tap::Tapped::Keys(Vec::new())),
        "the write observer must detect an accepted edit"
    );
    let after = disk_state(directory.path())?;
    assert_ne!(after.files, before.files);
    assert_ne!(journal(&after), journal(&before));
    eprintln!(
        "one accepted edit: logical file growth={} bytes; application charge delta={} bytes",
        i128::from(after.logical) - i128::from(before.logical),
        store.usage()?.bytes.iter().sum::<u64>() - usage.bytes.iter().sum::<u64>()
    );
    Ok(())
}
