use custom_logger::*;
use mirror_error::MirrorError;
use mirror_utils::{
    fs_copy, fs_handler, fs_open_or_create, keepalive, read_and_parse_metadata,
    read_and_parse_oci_manifest,
};
use serde_derive::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::thread::{sleep, spawn};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
pub struct MirrorStats {
    #[serde(rename = "blobCount")]
    pub blob_count: i16,
    #[serde(rename = "blobSize")]
    pub blob_size: u64,
    #[serde(rename = "manifestCount")]
    pub manifest_count: i16,
    #[serde(rename = "manifestSize")]
    pub manifest_size: u64,
    #[serde(rename = "metadataCount")]
    pub metadata_count: i16,
    #[serde(rename = "metadataSize")]
    pub metadata_size: u64,
}

pub async fn create_tar(
    log: &Logging,
    base_dir: String,
    archive_size: i64,
    vec_arch: Vec<String>,
) -> Result<bool, MirrorError> {
    // create the relevant directories
    fs_handler("tmp-blobs-dir".to_string(), "create_dir", None).await?;
    fs_handler(base_dir.clone() + &"/artifacts", "create_dir", None).await?;
    fs_handler("tmp-manifest-dir/operator".to_string(), "create_dir", None).await?;
    fs_handler(
        "tmp-manifest-dir/ocp-release".to_string(),
        "create_dir",
        None,
    )
    .await?;

    let metadata_files: Vec<&str> = vec![
        "release-image-reference.json",
        "additional-image-reference.json",
        "operator-image-reference.json",
    ];

    let mut blob_count = 0;
    let mut manifest_count = 0;
    let mut current_size: i64 = 0;
    let mut total_size: i64 = 0;
    let mut sequence = 1;

    for file in metadata_files.iter() {
        // read the mirror-metadata files
        log.info(&format!("[create_tar] reading metadata file {:?}", file));
        let file_path = format!("{}/{}/{}", base_dir, "mirror-metadata", file);
        if Path::new(&file_path).exists() {
            let metadata = read_and_parse_metadata(file_path)?;
            for img in metadata.clone().iter() {
                if img.manifest_type == "manifest" && vec_arch.contains(&img.arch.to_string()) {
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
                    log.trace(&format!("[create_tar] manifest_file {}", manifest_file));
                    log.debug(&format!(
                        "[create_tar] copying blobs for image {:#?}",
                        img.name
                    ));
                    let m_data = read_and_parse_oci_manifest(manifest_file.clone())?;
                    let to = format!("tmp-blobs-dir/{}/blob", img.namespace.clone());
                    fs_handler(to.clone(), "create_dir", None).await?;
                    let manifest = m_data.clone();
                    for layer in manifest.clone().layers.unwrap().iter() {
                        let digest = layer.digest.split(":").nth(1).unwrap();
                        let from = base_dir.clone()
                            + "/blobs-store/"
                            + &digest[..2]
                            + &String::from("/")
                            + digest;
                        let to_file = format!("{}/{}", to.clone(), digest);
                        log.trace(&format!(
                            "[create_tar] copy from {:#?} to {:#?}",
                            from,
                            to_file.clone()
                        ));
                        if !Path::new(&to_file).exists() {
                            fs_copy(from.clone(), to_file.clone()).await?;
                            blob_count += 1;
                            current_size = current_size + layer.size;
                        }
                    }
                    let cfg_digest_sha = manifest.clone().config.unwrap().digest;
                    let cfg_digest = cfg_digest_sha.split(":").nth(1).unwrap();
                    log.debug(&format!("[create_tar] config digest {:#?}", cfg_digest));
                    let from = base_dir.clone()
                        + "/blobs-store/"
                        + &cfg_digest[..2]
                        + &String::from("/")
                        + &cfg_digest;
                    let to_file = format!("{}/{}", to.clone(), cfg_digest);
                    log.trace(&format!(
                        "[create_tar] copy from {:#?} to {:#?}",
                        from,
                        to_file.clone()
                    ));
                    if !Path::new(&to_file).exists() {
                        fs_copy(from.clone(), to_file.clone()).await?;
                        blob_count += 1;
                        current_size = current_size + manifest.clone().config.unwrap().size;
                    }
                    if current_size >= archive_size {
                        total_size = total_size + current_size;
                        create_new_blobs_tar(base_dir.clone(), sequence).await?;
                        sequence += 1;
                        current_size = 0;
                    }
                    let to = format!("{}/{}/digest/", img.mirror_type, img.namespace);
                    let to_dir = format!("tmp-manifest-dir/{}", to.clone());
                    fs_handler(to_dir.clone(), "create_dir", None).await?;
                    let to_file: String;
                    if img.tag.is_some() {
                        to_file = format!("{}-{}.json", img.tag.as_ref().unwrap(), img.arch);
                    } else {
                        to_file = format!("{}-{}.json", img.digest, img.arch);
                    }
                    fs_copy(manifest_file.clone(), format!("{}/{}", to_dir, to_file)).await?;
                    manifest_count += 1;
                }
            }
        } else {
            log.warn(&format!("[create_tar] no refences for {}", file));
        }
    }
    log.debug(&format!(
        "total manifest count (arch filtered )           : {}",
        manifest_count
    ));
    log.debug(&format!(
        "total blob count (arch and duplicates filtered) : {}",
        blob_count
    ));
    log.ex("  [create_tar] building blob archive/s ");
    let (keepalive_send, keepalive_recv) = keepalive::channel();
    let join_handle = spawn(move || {
        let counter = 0;
        let spinner = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        while keepalive_recv.is_alive() {
            for x in 0..9 {
                println!("\x1b[1A \x1b[38C{}", spinner[x]);
                sleep(Duration::from_millis(50));
            }
        }
        counter
    });
    create_new_blobs_tar(base_dir.clone(), sequence).await?;
    drop(keepalive_send);
    let _ = join_handle.join().unwrap();
    println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
    let tar_manifest =
        File::create(base_dir.clone() + &"/artifacts/mirror-manifests.tar".to_string()).unwrap();
    let mut tar_m = tar::Builder::new(tar_manifest);
    let manifest_size = fs_extra::dir::get_size("tmp-manifest-dir").unwrap();
    log.ex("  [create_tar] building manifest archive ");
    tar_m.append_dir_all(".", "tmp-manifest-dir").unwrap();
    tar_m.finish().expect("should flush manifest contents");
    println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
    let tar_meta =
        File::create(base_dir.clone() + &"/artifacts/mirror-metadata.tar".to_string()).unwrap();
    let mut tar_md = tar::Builder::new(tar_meta);
    let src_dir = format!("{}/{}", base_dir.clone(), "mirror-metadata");
    let metadata_size = fs_extra::dir::get_size(src_dir.clone()).unwrap();
    log.ex("  [create_tar] building metadata archive ");
    tar_md.append_dir_all(".", src_dir.clone()).unwrap();
    tar_m.finish().expect("should flush metadata contents");
    println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
    let ms = MirrorStats {
        blob_count,
        blob_size: total_size as u64,
        manifest_count,
        manifest_size,
        metadata_count: 3,
        metadata_size,
    };
    let serialized_data = serde_json::to_string(&ms).unwrap();
    let ms_file = format!("{}/{}", base_dir.clone(), "/artifacts/mirror-stats.json");
    fs_handler(ms_file, "write", Some(serialized_data)).await?;
    fs_handler("tmp-manifest-dir".to_string(), "remove_dir", None).await?;
    fs_handler("tmp-blobs-dir".to_string(), "remove_dir", None).await?;
    Ok(true)
}
async fn create_new_blobs_tar(dir: String, sequence: i64) -> Result<(), MirrorError> {
    let tar_sequence = format!(
        "{}/{}-{:0>4}.tar",
        dir.clone(),
        "artifacts/mirror-blobs",
        sequence
    );
    let tar_blobs = fs_open_or_create(tar_sequence, true).await?;
    let mut tar_b = tar::Builder::new(tar_blobs);
    // add all the contents to the blobs
    tar_b.append_dir_all(".", "tmp-blobs-dir").unwrap();
    tar_b.finish().expect("should flush blob contents");
    // cleanup
    fs_handler("tmp-blobs-dir".to_string(), "remove_dir", None).await?;
    fs_handler("tmp-blobs-dir".to_string(), "create_dir", None).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_tar_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };

        macro_rules! aw {
            ($e:expr) => {
                tokio_test::block_on($e)
            };
        }
        let vec_arch = vec![
            "amd64".to_string(),
            "arm64".to_string(),
            "ppc64le".to_string(),
            "390x".to_string(),
        ];
        let res = aw!(create_tar(
            log,
            "test-artifacts/do-not-delete".to_string(),
            1,
            vec_arch.clone()
        ));
        assert_eq!(res.is_ok(), true);
        let res = aw!(create_tar(
            log,
            "test-artifacts/missing-files".to_string(),
            1,
            vec_arch.clone()
        ));
        assert_eq!(res.is_ok(), true);
    }
}
