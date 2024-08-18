use crate::error::handler::MirrorError;
use crate::image::utils::{parse_image, process_fb_image};
use crate::MirrorImageInfo;
use custom_logger::*;
use mirror_catalog_index::find_dir;
use serde_derive::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::fs::*;
use std::io::Read;
use std::process::Command;

#[derive(Serialize, Deserialize)]
pub struct CatalogHeader {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "defaultChannel")]
    pub default_channel: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CatalogCopyInfo {
    #[serde(rename = "catalog")]
    pub catalog: String,
    #[serde(rename = "package")]
    pub package: String,
    #[serde(rename = "channel")]
    pub channel: String,
}

#[derive(Debug, Clone)]
pub struct ImplCatalogBuildInterface {}

pub trait CatalogBuildInterface {
    async fn build_catalog(
        &self,
        log: &Logging,
        base_dir: String,
        catalogs: Vec<CatalogCopyInfo>,
    ) -> Result<Vec<MirrorImageInfo>, MirrorError>;
}

impl CatalogBuildInterface for ImplCatalogBuildInterface {
    async fn build_catalog(
        &self,
        log: &Logging,
        base_dir: String,
        catalogs: Vec<CatalogCopyInfo>,
    ) -> Result<Vec<MirrorImageInfo>, MirrorError> {
        let mut vec_mii: Vec<MirrorImageInfo> = Vec::new();
        for catalog in catalogs.iter() {
            let ir = parse_image(log, catalog.catalog.clone());
            let res_cfc = create_filtered_config(log, base_dir.clone(), catalog.clone()).await;
            if res_cfc.is_err() {
                return Err(res_cfc.err().unwrap());
            }
            fs::copy(
                "hack/rebuild-catalog.containerfile",
                "rebuild-catalog.containerfile",
            )
            .expect("should copy containerfile");
            // TODO: should iterate over each catalog
            let build = build(
                log,
                catalog.catalog.clone(),
                "rebuild-catalog.containerfile".to_string(),
            );
            if build.is_err() {
                return Err(build.err().unwrap());
            }
            let save = save(log, catalog.catalog.clone(), ir.name.clone());
            if save.is_err() {
                return Err(save.err().unwrap());
            }
            // finally save blobs and manifests to folders and data structure
            let res_fbi = process_fb_image(
                base_dir.clone(),
                ir.name,
                ir.namespace,
                ir.version,
                "operator".to_string(),
            );
            if res_fbi.is_err() {
                return Err(res_fbi.err().unwrap());
            }
            vec_mii.insert(0, res_fbi.unwrap().clone());
            cleanup();
        }
        Ok(vec_mii)
    }
}

async fn create_filtered_config(
    log: &Logging,
    base_dir: String,
    catalog: CatalogCopyInfo,
) -> Result<(), MirrorError> {
    // create a temp working folder
    let ir = parse_image(log, catalog.catalog.clone());
    let working_dir_cache = format!(
        "{}/{}/{}/{}/cache",
        base_dir.clone(),
        ir.name.clone(),
        ir.version.clone(),
        "amd64".to_string(),
    );

    let config_dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;

    let configs = format!("{}/{}", "configs", catalog.package);
    fs::create_dir_all(configs.clone()).expect("should create temp configs dir");

    let file_name = format!("{}/{}/catalog.json", config_dir, catalog.package);
    // Open the path in read-only mode, returns `Result()`
    let f = File::open(&file_name);
    if f.is_err() {
        let err = MirrorError::new(&format!(
            "reading declarative config {:?}",
            f.err().unwrap().to_string().to_lowercase()
        ));
        return Err(err);
    }

    // do some funky splicing
    // Read the file contents into a string, returns `io::Result<usize>`
    let mut s = String::new();
    f.unwrap().read_to_string(&mut s).expect("should read");
    let chunks = s.split("}\n{");
    let l = chunks.clone().count();
    for (pos, item) in chunks.clone().enumerate() {
        // first chunk
        if pos == 0 {
            let update = item.to_string() + "}";
            let header = parse_json_header(update.clone()).unwrap();
            let file_name = format!("{}/{}-{}.json", configs.clone(), header.name, header.schema);
            write(file_name, update).expect("should write spliced catalog (first section)");
        }
        // last chunk
        if pos == l - 1 {
            let update = "{".to_string() + item;
            let header = parse_json_header(update.clone()).unwrap();
            let file_name = format!("{}/{}-{}.json", configs, header.name, header.schema);
            write(file_name, update.clone()).expect("should write spliced catalog (last section)");
        }
        // everything in between
        if pos > 0 && pos <= l - 2 {
            let update = "{".to_string() + item + "}";
            let header = parse_json_header(update.clone()).unwrap();
            let file_name = format!("{}/{}-{}.json", configs, header.name, header.schema);
            write(file_name, update).expect("should write spliced catalog (mid section)");
        }
    }

    // we now have all the relevant catalog files
    // take the package, channel and bundle
    // check if the channel != defaultChannel
    log.info(&format!("DEBUG LMZ {}", catalog.channel));
    let package = format!("{}/{}-{}.json", configs, catalog.package, "olm.package");
    let pkg_data = fs::read_to_string(package.clone()).expect("should read package file");
    let pkg_json = parse_json_header(pkg_data.clone()).unwrap();
    if pkg_json.default_channel.is_some() {
        let default_channel = pkg_json.default_channel.unwrap();
        if catalog.channel != default_channel {
            let updated_pkg_data = pkg_data.replace(&default_channel, &catalog.channel);
            fs::write(package.clone(), updated_pkg_data).expect("update default channel")
        }
    }
    Ok(())
}

fn build(log: &Logging, image: String, container_file: String) -> Result<(), MirrorError> {
    let output = Command::new("podman")
        .arg("build")
        //.arg("-q")
        .arg("-t")
        .arg(&image)
        .arg("-f")
        .arg(&container_file)
        .output()
        .expect("failed to execute process");

    if output.status.success() {
        log.info("build image completed successfully");
    }
    log.info(&format!(
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    ));
    log.info(&format!(
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    ));

    assert!(output.status.success());
    Ok(())
}

fn save(log: &Logging, image: String, output_file: String) -> Result<(), MirrorError> {
    let output = Command::new("podman")
        .arg("save")
        .arg("--format")
        .arg("docker-dir")
        //.arg("-m")
        .arg("-o")
        .arg(output_file)
        .arg(image)
        .output()
        .expect("failed to execute process");

    if output.status.success() {
        log.info("save image (v2d2) to disk completed successfully");
    }
    log.info(&format!(
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    ));
    log.info(&format!(
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    ));

    assert!(output.status.success());
    Ok(())
}

fn cleanup() {
    // finally cleanup
    //fs::remove_dir_all("configs").expect("should clean configs");
    fs::remove_file("rebuild-catalog.containerfile").expect("should clean containerfile");
    // TODO: copy blobs and manifest
    //fs::remove_dir_all("redhat-catalog").expect("should clean redhat-catalog");
}

// parse the manifest json for operator indexes only
pub fn parse_json_header(data: String) -> Result<CatalogHeader, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: CatalogHeader = serde_json::from_str(&data)?;
    Ok(root)
}
