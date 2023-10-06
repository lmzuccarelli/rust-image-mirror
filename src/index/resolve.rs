// module resolve

use flate2::read::GzDecoder;
use std::collections::HashSet;
//use std::fs;
use std::fs::File;
use std::path::Path;
use tar::Archive;

use crate::api::schema::*;
use crate::batch::copy::*;
use crate::log::logging::*;

// untar layers in directory denoted by parameter 'dir'
pub async fn untar_layers(log: &Logging, dir: String, layers: Vec<FsLayer>) {
    // clean all duplicates
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for img in layers.iter() {
        // truncate sha256:
        let truncated_image = img.blob_sum.split(":").nth(1).unwrap();
        if !seen.contains(truncated_image) {
            seen.insert(truncated_image);
            images.push(img.blob_sum.clone());
        }
    }

    // read directory, iterate each file and untar
    for path in images.iter() {
        let blob = path.split(":").nth(1).unwrap();
        let cache_file = dir.clone() + "/" + &blob[..6];
        if !Path::new(&cache_file).exists() {
            let file = get_blobs_file(blob);
            let tar_gz = File::open(file).expect("could not open file");
            let tar = GzDecoder::new(tar_gz);
            let mut archive = Archive::new(tar);
            // should always be a sha256 string
            log.info(&format!("untarring file {} ", &blob[..6]));
            // we are really interested in either the configs or release-images directories
            match archive.unpack(cache_file) {
                Ok(arch) => arch,
                Err(error) => {
                    let msg = format!("skipping this error : {} ", &error.to_string());
                    log.warn(&msg);
                }
            };
        } else {
            log.debug(&format!("cache exists {}", cache_file));
        }
    }
}

// parse_image_index - best attempt to parse image index and return catalog reference
pub fn parse_image_index(log: &Logging, operators: Vec<Operator>) -> Vec<ImageReference> {
    let mut image_refs = vec![];
    for ops in operators.iter() {
        let img = ops.catalog.clone();
        log.trace(&format!("catalogs {:#?}", ops.catalog));
        let mut i = img.split(":");
        let index = i.nth(0).unwrap();
        let mut hld = index.split("/");
        let ver = i.nth(0).unwrap();
        let ir = ImageReference {
            registry: hld.nth(0).unwrap().to_string(),
            namespace: hld.nth(0).unwrap().to_string(),
            name: hld.nth(0).unwrap().to_string(),
            version: ver.to_string(),
            packages: ops.packages.clone().unwrap(),
        };

        log.debug(&format!("image reference {:#?}", img));
        image_refs.insert(0, ir);
    }
    image_refs
}

// get_cache_dir
pub fn get_cache_dir(name: String, version: String) -> String {
    let mut file = String::from("working-dir/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/");
    file.push_str(&"cache");
    file
}
