use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::RunError;

pub(super) fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), RunError> {
    for entry in fs::read_dir(directory).map_err(|error| {
        RunError(format!(
            "could not read artifact directory {}: {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| RunError(format!("could not read artifact: {error}")))?;
        let file_type = entry.file_type().map_err(|error| {
            RunError(format!(
                "could not inspect artifact {}: {error}",
                entry.path().display()
            ))
        })?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, RunError> {
    let mut file = File::open(path)
        .map_err(|error| RunError(format!("could not open {}: {error}", path.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| RunError(format!("could not read {}: {error}", path.display())))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn verify_manifest(directory: &Path, manifest: &Value) -> Result<(), RunError> {
    let entries = manifest["artifacts"]
        .as_array()
        .ok_or_else(|| RunError("manifest artifacts must be an array".to_owned()))?;
    let mut referenced = BTreeSet::new();
    for entry in entries {
        let relative = entry["path"]
            .as_str()
            .ok_or_else(|| RunError("manifest artifact path must be a string".to_owned()))?;
        if !referenced.insert(relative.to_owned()) {
            return Err(RunError(format!(
                "manifest references artifact {relative} more than once"
            )));
        }
        let path = directory.join(relative);
        let bytes = fs::metadata(&path)
            .map_err(|error| RunError(format!("could not stat {}: {error}", path.display())))?
            .len();
        if entry["bytes"].as_u64() != Some(bytes)
            || entry["sha256"].as_str() != Some(sha256_file(&path)?.as_str())
        {
            return Err(RunError(format!(
                "manifest verification failed for {}",
                path.display()
            )));
        }
    }
    let mut files = Vec::new();
    collect_files(directory, &mut files)?;
    let actual = files
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("manifest.json"))
        .count();
    if referenced.len() != actual {
        return Err(RunError(format!(
            "manifest references {} artifacts but staging contains {actual}",
            referenced.len()
        )));
    }
    Ok(())
}

pub(super) fn sync_tree(directory: &Path) -> Result<(), RunError> {
    let mut files = Vec::new();
    collect_files(directory, &mut files)?;
    for path in files {
        File::open(&path)
            .and_then(|file| file.sync_all())
            .map_err(|error| RunError(format!("could not sync {}: {error}", path.display())))?;
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn sync_directory(directory: &Path) -> Result<(), RunError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            RunError(format!(
                "could not sync directory {}: {error}",
                directory.display()
            ))
        })
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_directory: &Path) -> Result<(), RunError> {
    Ok(())
}

pub(super) fn utc_timestamp() -> Result<String, RunError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RunError(format!("system clock precedes Unix epoch: {error}")))?
        .as_secs();
    let days = i64::try_from(seconds / 86_400)
        .map_err(|_| RunError("current date is out of range".to_owned()))?;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
