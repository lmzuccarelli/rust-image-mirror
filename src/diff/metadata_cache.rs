use crate::log::logging::*;
use crate::manifests::catalogs::parse_json_manifest_operator;
use chrono::NaiveDateTime;
// use exitcode;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::time::SystemTime;
use tempdir::TempDir;
use walkdir::WalkDir;

pub fn get_metadata_dirs_by_date(log: &Logging, dir: String, date: String) -> HashSet<String> {
    let mut valid_dirs = HashSet::new();
    let new_date = date + &String::from(" 00:00:00");
    log.info(&format!("date {:#?}", new_date));
    let date_only = NaiveDateTime::parse_from_str(&new_date, "%Y/%m/%d %H:%M:%S");
    match date_only {
        Ok(val) => val,
        Err(_) => {
            log.error("date expected to be in yyyy/mm/dd format");
            //std::process::exit(exitcode::USAGE);
            panic!("date format is incorrect, unable to continue")
        }
    };

    let date_unix = NaiveDateTime::timestamp(&date_only.unwrap());
    log.info(&format!("date unix {:#?}", date_unix));
    for e in WalkDir::new(dir.clone().to_string())
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

pub fn get_metadata_dirs_incremental(log: &Logging, dir: String) -> HashSet<String> {
    let mut valid_dirs = HashSet::new();
    for e in WalkDir::new(dir.clone()).into_iter().filter_map(|e| e.ok()) {
        if e.path().is_dir() {
            let dir = e.path().display().to_string();
            if (dir.contains("operators") || dir.contains("release"))
                && (Path::new(&(dir.clone() + &"/manifest.json".to_string())).exists()
                    || Path::new(&(dir.clone() + &"/manifest-list.json".to_string())).exists())
            {
                log.debug(&format!("valid metadata directories {:#?}", dir));
                valid_dirs.insert(dir);
            }
        }
    }
    valid_dirs
}

pub fn create_diff_tar(
    log: &Logging,
    tar_file: String,
    base_dir: String,
    dirs: Vec<&std::string::String>,
    config: String,
) -> Result<bool, Box<dyn std::error::Error>> {
    let tmp_dir = TempDir::new("tmp-diff-tar")?;
    // working-dir/blobs-store
    let from_base = base_dir.clone();
    fs::create_dir_all(tmp_dir.path().join("metadata"))?;
    fs::create_dir_all(tmp_dir.path().join("blobs"))?;
    for x in dirs {
        // open the manifest file/s (could be more than one - multiarch)
        log.info(&format!("component directory {:#?}", x.to_string()));
        fs::create_dir_all(tmp_dir.path().join(x.to_string()))?;
        log.trace(&format!("each dir vector {}", x));

        for entry in fs::read_dir(x.to_string())? {
            let entry = entry?;
            let path = entry.path();
            let from = path.clone().into_os_string().into_string().unwrap();
            let to = tmp_dir.path().join(from.clone());
            fs::copy(from.clone(), to).unwrap();
            // parse the file contents, read it into a Manifest struct
            let s = fs::read_to_string(from.clone())?;
            if !from.contains("list") & !from.contains("catalog") {
                log.trace(&format!("from {}", from));
                let mnfst = parse_json_manifest_operator(s.clone()).unwrap();
                for layer in mnfst.layers.unwrap().iter() {
                    let digest = layer.digest.split(":").nth(1).unwrap();
                    log.hi(&format!("layer to copy {:#?}", digest));
                    let to_dir = String::from("blobs/") + digest;
                    let from = from_base.clone() + &digest[..2] + &String::from("/") + digest;
                    let to = tmp_dir.path().join(&to_dir);
                    log.trace(&format!("copy from {:#?} to {:#?}", from, to));
                    fs::copy(from, to).unwrap();
                }
                if mnfst.config.is_some() {
                    let mnfst_config = mnfst.config.unwrap();
                    let cfg_digest = mnfst_config.digest.split(":").nth(1).unwrap();
                    log.lo(&format!("config to copy {:#?}", cfg_digest));
                    let from =
                        from_base.clone() + &cfg_digest[..2] + &String::from("/") + cfg_digest;
                    let to_dir = String::from("blobs/") + cfg_digest;
                    let to = tmp_dir.path().join(&to_dir);
                    log.trace(&format!("copy from {:#?} to {:#?}", from, to));
                    fs::copy(from, to).unwrap();
                    log.trace("config copied to tmp dir");
                }
            }
        }
    }

    log.trace("building tar ball ....");
    // finally copy over the current imagesetconfig used
    fs::write(tmp_dir.path().join("metadata/isc.yaml"), config.clone())
        .expect("should write isc.yaml file");
    log.trace(&format!("imagesetconfig written {}", config));
    // create the tar
    let tar_gz = File::create(tar_file.clone()).unwrap();
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);
    // add all the contents to the tar
    tar.append_dir_all(".", tmp_dir.path()).unwrap();
    tmp_dir.close().unwrap();
    Ok(true)
}

#[cfg(test)]
mod tests {

    // this brings everything from parent's scope into this scope
    use super::*;

    #[test]
    fn get_metadata_dirs_incremental_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };
        let mut hs = HashSet::new();
        hs.insert(String::from(
            "test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1",
        ));
        let res = get_metadata_dirs_incremental(
            log,
            String::from("test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1"),
        );
        assert_eq!(res, hs);
    }

    #[test]
    fn get_metadata_dirs_by_date_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };
        let mut hs = HashSet::new();
        hs.insert(String::from(
            "test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1",
        ));
        let res = get_metadata_dirs_by_date(
            log,
            String::from("test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1"),
            String::from("2023/08/01"),
        );
        assert_eq!(res, hs);
    }

    #[test]
    #[should_panic]
    fn get_metadata_dirs_by_date_fail() {
        let log = &Logging {
            log_level: Level::INFO,
        };
        let mut hs = HashSet::new();
        hs.insert(String::from("test-artifacts/operators"));
        let res =
            get_metadata_dirs_by_date(log, String::from("test-artifacts"), String::from("/08/01"));
        assert_eq!(res, hs);
    }

    #[test]
    fn create_diff_tar_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };
        let mnfst_dir =
            &"test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1/".to_string();
        let files = vec![mnfst_dir];
        let res = create_diff_tar(
            log,
            String::from("test-diff.tar.gz"),
            String::from("test-artifacts/blobs-store/"),
            files.clone(),
            String::from("imagesetconfig"),
        );
        let exists = fs::metadata("test-diff.tar.gz").is_ok();
        assert_eq!(exists, true);
        fs::remove_file("test-diff.tar.gz").expect("should delete file");
        log.info(&format!("return value {:#?}", res));
    }
}
