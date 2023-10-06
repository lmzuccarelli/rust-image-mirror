use crate::log::logging::*;
use crate::manifests::catalogs::parse_json_manifest_operator;
use chrono::NaiveDateTime;
use exitcode;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::time::SystemTime;
use tempdir::TempDir;
use walkdir::WalkDir;

pub fn get_metadata_dirs_by_date(log: &Logging, date: String) -> HashSet<String> {
    let mut valid_dirs = HashSet::new();
    let new_date = date + &String::from(" 00:00:00");
    log.info(&format!("date {:#?}", new_date));
    let date_only = NaiveDateTime::parse_from_str(&new_date, "%Y/%m/%d %H:%M:%S");
    match date_only {
        Ok(val) => val,
        Err(_) => {
            log.error("date expected to be in yyyy/mm/dd format");
            std::process::exit(exitcode::USAGE);
        }
    };

    let date_unix = NaiveDateTime::timestamp(&date_only.unwrap());
    log.info(&format!("date unix {:#?}", date_unix));
    for e in WalkDir::new("working-dir/".to_string())
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.path().is_dir() {
            let dir = e.path().display().to_string();
            let metadata = fs::metadata(&dir).unwrap();
            let created = metadata
                .created()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap();
            if (dir.contains("operators") || dir.contains("release"))
                && (Path::new(&(dir.clone() + &"/manifest.json".to_string())).exists()
                    || Path::new(&(dir.clone() + &"/manifest-list.json".to_string())).exists())
                && created.as_secs() > date_unix as u64
            {
                log.hi(&format!(
                    "timestamp {:#?} for dir {:#?} ",
                    created.as_secs(),
                    dir
                ));
                valid_dirs.insert(dir);
            }
        }
    }
    valid_dirs
}

pub fn get_metadata_dirs_incremental(log: &Logging) -> HashSet<String> {
    let mut valid_dirs = HashSet::new();
    for e in WalkDir::new("working-dir/".to_string())
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.path().is_dir() {
            let dir = e.path().display().to_string();
            if (dir.contains("operators") || dir.contains("release"))
                && (Path::new(&(dir.clone() + &"/manifest.json".to_string())).exists()
                    || Path::new(&(dir.clone() + &"/manifest-list.json".to_string())).exists())
            {
                log.info(&format!("valid metadata directories {:#?}", dir));
                valid_dirs.insert(dir);
            }
        }
    }
    valid_dirs
}

pub fn create_diff_tar(
    log: &Logging,
    dirs: Vec<&std::string::String>,
    config: String,
) -> Result<bool, Box<dyn std::error::Error>> {
    log.info("creating mirror_diff.tar.gz");
    let tmp_dir = TempDir::new("tmp-diff-tar")?;
    let from_base = String::from("working-dir/blobs-store/");
    fs::create_dir_all(tmp_dir.path().join("metadata"))?;
    fs::create_dir_all(tmp_dir.path().join("blobs"))?;
    for x in dirs {
        // open the manifest file/s (could be more than one - multiarch)
        log.info(&format!("component directory {:#?}", x.to_string()));
        fs::create_dir_all(tmp_dir.path().join(x.to_string()))?;
        for entry in fs::read_dir(x.to_string())? {
            let entry = entry?;
            let path = entry.path();
            let from = path.clone().into_os_string().into_string().unwrap();
            let to = tmp_dir.path().join(from.clone());
            // copy the manifest
            fs::copy(from.clone(), to)?;
            // parse the file contents, read it into a Manifest struct
            let s = fs::read_to_string(from.clone())?;
            log.trace(&format!("from {}", from));
            if !from.contains("list") {
                let mnfst = parse_json_manifest_operator(s.clone()).unwrap();
                for layer in mnfst.layers.unwrap().iter() {
                    let digest = layer.digest.split(":").nth(1).unwrap();
                    log.info(&format!("layer to copy {:#?}", digest));
                    let to_dir = String::from("blobs/") + digest;
                    let from = from_base.clone() + &digest[..2] + &String::from("/") + digest;
                    let to = tmp_dir.path().join(&to_dir);
                    log.trace(&format!("copy from {:#?} to {:#?}", from, to));
                    fs::copy(from, to)?;
                }
                let mnfst_config = mnfst.config.unwrap();
                let cfg_digest = mnfst_config.digest.split(":").nth(1).unwrap();
                log.lo(&format!("config to copy {:#?}", cfg_digest));
                let from = from_base.clone() + &cfg_digest[..2] + &String::from("/") + cfg_digest;
                let to_dir = String::from("blobs/") + cfg_digest;
                let to = tmp_dir.path().join(&to_dir);
                fs::copy(from, to)?;
            }
        }
    }
    // finally copy over the current imagesetconfig used
    fs::write(tmp_dir.path().join("metadata/isc.yaml"), config)?;
    // create the tar
    let tar_gz = File::create("mirror_diff.tar.gz")?;
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);
    // add all the contents to the tar
    tar.append_dir_all(".", tmp_dir.path())?;
    tmp_dir.close()?;
    Ok(true)
}
