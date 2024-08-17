use crate::error::handler::MirrorError;
use crate::image::utils::*;
use custom_logger::*;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use tar::Archive;

#[derive(Serialize, Deserialize)]
pub struct GenerateClusterResources {
    #[serde(rename = "from")]
    from_dir: String,
}

#[derive(Serialize, Deserialize)]
pub struct ImageTagMirrorSet {
    #[serde(rename = "apiVersion")]
    api_version: String,

    #[serde(rename = "kind")]
    kind: String,

    #[serde(rename = "metadata")]
    metadata: Metadata,

    #[serde(rename = "spec")]
    spec: TagSpec,

    #[serde(rename = "status")]
    status: Status,
}

#[derive(Serialize, Deserialize)]
pub struct ImageDigestMirrorSet {
    #[serde(rename = "apiVersion")]
    api_version: String,

    #[serde(rename = "kind")]
    kind: String,

    #[serde(rename = "metadata")]
    metadata: Metadata,

    #[serde(rename = "spec")]
    spec: DigestSpec,

    #[serde(rename = "status")]
    status: Status,
}

#[derive(Serialize, Deserialize)]
pub struct CatalogSource {
    #[serde(rename = "apiVersion")]
    api_version: String,

    #[serde(rename = "kind")]
    kind: String,

    #[serde(rename = "metadata")]
    metadata: Metadata,

    #[serde(rename = "spec")]
    spec: CatalogSourceSpec,

    #[serde(rename = "status")]
    status: Status,
}

#[derive(Serialize, Deserialize)]
pub struct Metadata {
    #[serde(rename = "name")]
    name: String,

    #[serde(rename = "namespace")]
    namespace: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct TagSpec {
    #[serde(rename = "imageTagMirrors")]
    image_tag_mirrors: Vec<MirrorSource>,
}

#[derive(Serialize, Deserialize)]
pub struct DigestSpec {
    #[serde(rename = "imageDigestMirrors")]
    image_digest_mirrors: Vec<MirrorSource>,
}

#[derive(Serialize, Deserialize)]
pub struct CatalogSourceSpec {
    #[serde(rename = "image")]
    image: String,

    #[serde(rename = "sourceType")]
    source_type: String,
}

#[derive(Serialize, Deserialize)]
pub struct MirrorSource {
    #[serde(rename = "mirrors")]
    mirrors: Vec<String>,

    #[serde(rename = "source")]
    source: String,
}

#[derive(Serialize, Deserialize)]
pub struct Status {}

#[derive(Serialize, Deserialize)]
pub struct ImageMirrorInfo {
    pub source: String,
    pub destination: String,
}

impl GenerateClusterResources {
    pub fn new(from_dir: String) -> GenerateClusterResources {
        GenerateClusterResources {
            from_dir: format!("{}/{}", from_dir, "mirror-metadata.tar"),
        }
    }

    pub fn untar_metadata(&self, log: &Logging) -> Result<(), MirrorError> {
        // read the tar file
        log.info(&format!(
            "reading from untarred contents {}",
            &self.from_dir
        ));
        let data = std::fs::File::open(&self.from_dir);
        if data.is_ok() {
            let f_res = fs::create_dir_all("tmp-metadata");
            if f_res.is_ok() {
                let mut archive = Archive::new(data.unwrap());
                for (_i, file) in archive.entries().unwrap().enumerate() {
                    let mut x = file.unwrap();
                    let f = x.path().unwrap();
                    let op_path = f.as_ref().to_string_lossy().to_string();
                    if op_path.clone().contains(".json") {
                        log.info(&format!("full file {}", op_path.clone()));
                        let res = x.unpack(format!("{}/{}", "tmp-metadata", op_path.clone()));
                        if res.is_err() {
                            let err = MirrorError::new(&format!(
                                "accessing archive entries {:?}",
                                res.err().unwrap().to_string().to_lowercase()
                            ));
                            return Err(err);
                        }
                    }
                }
            } else {
                let err = MirrorError::new(&format!(
                    "creating temp archive directory {:?}",
                    f_res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            let err = MirrorError::new(&format!(
                "reading archive {:?}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    pub fn clean_up(&self) -> Result<(), MirrorError> {
        let res = fs::remove_dir_all("tmp-metadata");
        if res.is_err() {
            let err = MirrorError::new(&format!(
                "cleaning temp dir {:?}",
                res.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    pub fn generate_idms_itms(
        &self,
        log: &Logging,
        dir: String,
        destination: String,
    ) -> Result<(), MirrorError> {
        // write initial header to file
        fs::create_dir_all(format!("{}/{}", dir, "cluster-resources"))
            .expect("should create cluster-resource directory");
        fs::write(
            dir.clone() + &"/cluster-resources/idms-image-mirror.yaml",
            "",
        )
        .expect("should write idms yaml");
        fs::write(
            dir.clone() + &"/cluster-resources/itms-image-mirror.yaml",
            "",
        )
        .expect("should write itms yaml");

        let vec_files: Vec<String> = vec![
            "release-image-reference.json".to_string(),
            "operator-image-reference.json".to_string(),
            "additional-image-reference.json".to_string(),
        ];
        let mut map_digest: HashMap<String, Vec<String>> = HashMap::new();
        let mut map_tag: HashMap<String, Vec<String>> = HashMap::new();
        log.info("updating idms file");
        for file in vec_files.iter() {
            let json = format!("{}/{}", "tmp-metadata", file);
            let data = fs::read_to_string(json.clone());
            if data.is_ok() {
                let rir = parse_json_metadata(data.unwrap());
                if rir.is_ok() {
                    for mi in rir.unwrap().iter() {
                        if mi.arch == "x86_64"
                            || mi.arch == "amd64" && mi.digest.contains("sha256:")
                        {
                            log.info(&format!("{}", mi.reference.clone()));
                            let img = parse_image(log, mi.reference.clone());
                            let key = format!("{}/{}", img.registry, img.namespace);
                            let dest = format!("{}/{}", destination.clone(), mi.namespace.clone());
                            map_digest.insert(key.clone(), vec![dest]);
                        }
                        if mi.arch == "x86_64"
                            || mi.arch == "amd64" && mi.tag.is_some() && mi.digest.len() == 0
                        {
                            log.debug(&format!("{}", mi.reference.clone()));
                            let img = parse_image(log, mi.reference.clone());
                            let key = format!("{}/{}", img.registry, img.namespace);
                            let dest = format!("{}/{}", destination.clone(), mi.namespace.clone());
                            map_tag.insert(key.clone(), vec![dest]);
                        }
                    }
                } else {
                    let err = MirrorError::new(&format!(
                        "parsing metatdata {:?}",
                        rir.err().unwrap().to_string().to_lowercase()
                    ));
                    return Err(err);
                }
            } else {
                let err = MirrorError::new(&format!(
                    "reading metatdata {:?}",
                    data.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
            process_itms_idms(
                dir.clone(),
                map_digest.clone(),
                file.to_string(),
                "idms".to_string(),
            );
            process_itms_idms(
                dir.clone(),
                map_digest.clone(),
                file.to_string(),
                "itms".to_string(),
            );
        }
        Ok(())
    }

    pub fn generate_catalog_source(
        &self,
        log: &Logging,
        dir: String,
        _catalog: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        log.info("generating catalogsource");
        fs::write(
            format!("{}/{}{}", &dir, &"/cluster-resources/cs-", "image.yaml"),
            "",
        )
        .expect("should create catalogsource file");

        //self.spec.image = catalog.replace(":", "-").replace(".", "-").to_string();
        //self.api_version = "config.openshift.io/v1".to_string();
        //self.kind = "CatalogSource".to_string();
        //self.metadata.namespace = Some("openshift-marketplace".to_string());
        //self.spec.source_type = "grpc".to_string();

        // write with yaml format
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(format!(
                "{}/{}{}",
                &dir, &"/cluster-resources/cs-", "image.yaml"
            ))
            .expect("Couldn't open file");
        serde_yaml::to_writer(file, &self).unwrap();
        Ok(())
    }
}

fn process_itms_idms(
    dir: String,
    map: HashMap<String, Vec<String>>,
    image_type: String,
    kind: String,
) {
    let mut vec_mirrors: Vec<MirrorSource> = Vec::new();
    for (k, v) in map.clone() {
        let mirrors = MirrorSource {
            mirrors: v,
            source: k.clone(),
        };
        vec_mirrors.insert(0, mirrors);
    }
    let metadata_name = match image_type.clone() {
        x if x.contains("release") => format!("{}-release-0", kind),
        x if x.contains("operator") => format!("{}-operator-0", kind),
        x if x.contains("additional") => format!("{}-generic-0", kind),
        _ => "none".to_string(),
    };
    let md = Metadata {
        name: metadata_name,
        namespace: None,
    };
    if kind == "idms" {
        let dgs = DigestSpec {
            image_digest_mirrors: vec_mirrors,
        };
        let idms = ImageDigestMirrorSet {
            api_version: "config.openshift.io/v1".to_string(),
            kind: "ImageDigestMirrorSet".to_string(),
            metadata: md,
            spec: dgs,
            status: Status {},
        };
        let serialized_data = serde_yaml::to_string(&idms).unwrap();
        let idms_file = format!(
            "{}/{}",
            dir.clone(),
            "cluster-resources/idms-image-mirror.yaml"
        );
        // append to the file
        let mut file_ref = OpenOptions::new()
            .append(true)
            .open(idms_file)
            .expect("unable to open file");
        let final_data = format!("{}\n{}", "---", serialized_data);
        file_ref
            .write_all(final_data.as_bytes())
            .expect("write failed");
    } else {
        let its = TagSpec {
            image_tag_mirrors: vec_mirrors,
        };

        let itms = ImageTagMirrorSet {
            api_version: "config.openshift.io/v1".to_string(),
            kind: "ImageTagMirrorSet".to_string(),
            metadata: md,
            spec: its,
            status: Status {},
        };
        let serialized_data = serde_yaml::to_string(&itms).unwrap();
        let itms_file = format!(
            "{}/{}",
            dir.clone(),
            "cluster-resources/itms-image-mirror.yaml"
        );
        // append to the file
        let mut file_ref = OpenOptions::new()
            .append(true)
            .open(itms_file)
            .expect("unable to open file");
        let final_data = format!("{}\n{}", "---", serialized_data);
        file_ref
            .write_all(final_data.as_bytes())
            .expect("write failed");
    }
}
