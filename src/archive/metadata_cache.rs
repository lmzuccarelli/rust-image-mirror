use crate::error::handler::MirrorError;
use crate::image::utils::*;
use custom_logger::*;
use mirror_copy::parse_json_manifest_operator;
use std::fs;
use std::fs::File;
use tempdir::TempDir;

pub fn create_tar(log: &Logging, base_dir: String) -> Result<bool, MirrorError> {
    // setup blobs temp dir
    let tmp_blobs_dir = TempDir::new("tmp-blobs-tar");
    let blobs_dir = tmp_blobs_dir.as_ref().unwrap();

    // setupe manifest temp dir
    let tmp_manifest_dir = TempDir::new("tmp-manifest-tar");
    let manifest_dir = tmp_manifest_dir.as_ref().unwrap();
    fs::create_dir_all(manifest_dir.path().join("operator"))
        .expect("should create operators folder");
    fs::create_dir_all(manifest_dir.path().join("release")).expect("should create release folder");

    let metadata_files: Vec<&str> = vec![
        "release-image-reference.json",
        "additional-image-reference.json",
        "operator-image-reference.json",
    ];

    for file in metadata_files.iter() {
        // read the mirror-metadata files
        log.mid(&format!("reading metadata file {:?}", file));
        let file_path = format!("{}/{}/{}", base_dir, "mirror-metadata", file);
        let data = fs::read_to_string(file_path).expect("should read metadata file");
        let op_imgrefs = parse_json_metadata(data);

        if op_imgrefs.is_ok() {
            for img in op_imgrefs.unwrap().iter() {
                if img.manifest_type == "manifest"
                    && (img.arch == "amd64" || img.arch == "x86_64" || img.arch == "all")
                {
                    let td: String;
                    if img.tag.is_some() && img.digest.len() == 0 {
                        td = format!("{}:{}", img.name.clone(), img.tag.as_ref().unwrap());
                    } else {
                        td = img.digest.clone();
                    }
                    let manifest_file = format!(
                        "{}/manifests/{}/{}-{}.json",
                        base_dir.clone(),
                        img.mirror_type,
                        td,
                        img.arch
                    );
                    log.info(&format!("manifest_file {}", manifest_file));
                    log.info(&format!("processing image {:#?}", img.name));
                    let m_data = fs::read_to_string(manifest_file.clone())
                        .expect("should read manifest file");
                    let mnfst = parse_json_manifest_operator(m_data);
                    if mnfst.is_ok() {
                        // component manifest
                        let manifest = mnfst.unwrap();
                        for layer in manifest.clone().layers.unwrap().iter() {
                            let digest = layer.digest.split(":").nth(1).unwrap();
                            let to = blobs_dir.path().join(img.namespace.clone()).join("blob");
                            fs::create_dir_all(to.clone()).expect("create dest dir");
                            let from = base_dir.clone()
                                + "/blobs-store/"
                                + &digest[..2]
                                + &String::from("/")
                                + digest;
                            let to_file = to.clone().join(digest);
                            log.debug(&format!("copy from {:#?} to {:#?}", from, to_file.clone()));
                            let res = fs::copy(from.clone(), to_file.clone());
                            if res.is_err() {
                                let msg = &format!(
                                    "{} {}",
                                    "copying layer blob file",
                                    res.err().unwrap().to_string().to_lowercase()
                                );
                                let err = MirrorError::new(msg);
                                return Err(err);
                            }
                            // add config
                            let cfg_digest_sha = manifest.clone().config.unwrap().digest;
                            let cfg_digest = cfg_digest_sha.split(":").nth(1).unwrap();
                            log.debug(&format!("config digest {:#?}", cfg_digest));
                            let from = base_dir.clone()
                                + "/blobs-store/"
                                + &cfg_digest[..2]
                                + &String::from("/")
                                + &cfg_digest;
                            let to_file = to.join(cfg_digest);
                            log.debug(&format!("copy from {:#?} to {:#?}", from, to_file.clone()));
                            let res = fs::copy(from.clone(), to_file.clone());
                            if res.is_err() {
                                let msg = &format!(
                                    "{} {}",
                                    "copying config blob file",
                                    res.err().unwrap().to_string().to_lowercase()
                                );
                                let err = MirrorError::new(msg);
                                return Err(err);
                            }
                            // finally add manifest to temp dir
                            let to = format!("{}/{}/digest/", img.mirror_type, img.namespace);
                            let to_dir = manifest_dir.path().join(to.clone());
                            fs::create_dir_all(to_dir.clone()).expect("create dest dir");
                            let to_file: String;
                            if img.tag.is_some() {
                                to_file =
                                    format!("{}-{}.json", img.tag.as_ref().unwrap(), img.arch);
                            } else {
                                to_file = format!("{}-{}.json", img.digest, img.arch);
                            }
                            let res = fs::copy(manifest_file.clone(), to_dir.join(to_file));
                            if res.is_err() {
                                let msg = &format!(
                                    "{} {}",
                                    "copying manifest file",
                                    res.err().unwrap().to_string().to_lowercase()
                                );
                                let err = MirrorError::new(msg);
                                return Err(err);
                            }
                        }
                    } else {
                        let err = MirrorError::new(&mnfst.err().unwrap().to_string());
                        return Err(err);
                    }
                }
            }
        } else {
            let err = MirrorError::new(&op_imgrefs.err().unwrap().to_string());
            return Err(err);
        }
    }

    log.ex(&format!(
        "building blob archive with size :    {:#?}",
        fs_extra::dir::get_size(blobs_dir).unwrap()
    ));

    // create the tars
    fs::create_dir_all(base_dir.clone() + &"/artifacts")
        .expect("should create artifacts directory");
    let tar_blobs =
        File::create(base_dir.clone() + &"/artifacts/mirror-blobs.tar".to_string()).unwrap();
    let mut tar_b = tar::Builder::new(tar_blobs);
    // add all the contents to the blobs
    tar_b
        .append_dir_all(".", tmp_blobs_dir.as_ref().unwrap().path())
        .unwrap();
    tar_b.finish().expect("should flush blob contents");
    tmp_blobs_dir.unwrap().close().unwrap();

    let tar_manifest =
        File::create(base_dir.clone() + &"/artifacts/mirror-manifests.tar".to_string()).unwrap();
    let mut tar_m = tar::Builder::new(tar_manifest);

    log.ex(&format!(
        "building manifest archive with size : {:#?}",
        fs_extra::dir::get_size(manifest_dir).unwrap()
    ));

    tar_m.append_dir_all(".", manifest_dir.path()).unwrap();
    tar_m.finish().expect("should flush manifest contents");
    tmp_manifest_dir.unwrap().close().unwrap();

    let tar_meta =
        File::create(base_dir.clone() + &"/artifacts/mirror-metadata.tar".to_string()).unwrap();
    let mut tar_md = tar::Builder::new(tar_meta);

    let src_dir = format!("{}/{}", base_dir.clone(), "mirror-metadata");
    log.ex(&format!(
        "building metadata archive with size : {:#?}",
        fs_extra::dir::get_size(src_dir.clone()).unwrap()
    ));

    tar_md.append_dir_all(".", src_dir.clone()).unwrap();
    tar_m.finish().expect("should flush metadata contents");

    Ok(true)
}

#[cfg(test)]
mod tests {

    // this brings everything from parent's scope into this scope
    //use super::*;

    /*
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
    */
}
