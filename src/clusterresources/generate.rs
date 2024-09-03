use custom_logger::*;
use mirror_error::MirrorError;
use mirror_utils::{fs_handler, parse_image, read_and_parse_metadata};
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use tar::Archive;

#[derive(Serialize, Deserialize)]
pub struct GenerateClusterResources {
    #[serde(rename = "from")]
    from_dir: String,
    #[serde(rename = "tarFile")]
    tar_file: String,
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
            from_dir: from_dir.clone(),
            tar_file: format!("{}/{}", from_dir, "mirror-metadata.tar"),
        }
    }

    pub async fn untar_metadata(&self, log: &Logging) -> Result<(), MirrorError> {
        // read the tar file
        log.info(&format!(
            "[untar_metadata] processing metadata tar {}",
            &self.from_dir
        ));
        let data = std::fs::File::open(&self.tar_file);
        if data.is_ok() {
            fs_handler(
                format!("{}/tmp-metadata", &self.from_dir),
                "create_dir",
                None,
            )
            .await?;
            let mut archive = Archive::new(data.unwrap());
            for (_i, file) in archive.entries().unwrap().enumerate() {
                let mut x = file.unwrap();
                let f = x.path().unwrap();
                let op_path = f.as_ref().to_string_lossy().to_string();
                if op_path.clone().contains(".json") {
                    log.debug(&format!("[untar_metadata] file {}", op_path.clone()));
                    let res = x.unpack(format!(
                        "{}/{}/{}",
                        &self.from_dir,
                        "tmp-metadata",
                        op_path.clone()
                    ));
                    if res.is_err() {
                        let err = MirrorError::new(&format!(
                            "[untar_metdata] accessing archive entries {}",
                            res.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
            }
        } else {
            let err = MirrorError::new(&format!(
                "[untar_metadata] reading archive {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    pub async fn clean_up(&self, _dir: String) -> Result<(), MirrorError> {
        let exists = Path::new(&format!("{}/tmp-metadata", &self.from_dir.clone())).exists();
        if exists {
            fs_handler(
                format!("{}/tmp-metadata", self.from_dir.clone()),
                "remove_dir",
                None,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn generate_idms_itms(
        &self,
        log: &Logging,
        _dir: String,
        destination: String,
    ) -> Result<(), MirrorError> {
        // write initial header to file
        fs_handler(
            format!("{}/{}", &self.from_dir, "cluster-resources"),
            "create_dir",
            None,
        )
        .await?;
        let vec_files: Vec<String> = vec![
            "release-image-reference.json".to_string(),
            "operator-image-reference.json".to_string(),
            "additional-image-reference.json".to_string(),
        ];
        let mut map_digest: HashMap<String, Vec<String>> = HashMap::new();
        let mut map_tag: HashMap<String, Vec<String>> = HashMap::new();
        log.info("[generate_idms_itms] cluster resources");
        for file in vec_files.iter() {
            let json = format!("{}/{}/{}", &self.from_dir.clone(), "tmp-metadata", file);
            if Path::new(&json).exists() {
                let rir = read_and_parse_metadata(json.clone())?;
                for mi in rir.clone().iter() {
                    if mi.arch == "x86_64" || mi.arch == "amd64" && mi.digest.contains("sha256:") {
                        log.debug(&format!("{}", mi.reference.clone()));
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
                process_itms_idms(
                    self.from_dir.clone(),
                    map_digest.clone(),
                    file.to_string(),
                    "idms".to_string(),
                )
                .await?;
                process_itms_idms(
                    self.from_dir.clone(),
                    map_digest.clone(),
                    file.to_string(),
                    "itms".to_string(),
                )
                .await?;
            } else {
                log.warn(&format!(
                    "[generate_idms_itms] not generating idms/itms : no reference found for {}",
                    file.clone()
                ));
            }
        }
        Ok(())
    }
    pub async fn generate_catalog_source(
        &self,
        log: &Logging,
        dir: String,
        _catalog: String,
    ) -> Result<(), MirrorError> {
        log.info("[generate_catalog_source] creating catalogsource");
        //self.spec.image = catalog.replace(":", "-").replace(".", "-").to_string();
        //self.api_version = "config.openshift.io/v1".to_string();
        //self.kind = "CatalogSource".to_string();
        //self.metadata.namespace = Some("openshift-marketplace".to_string());
        //self.spec.source_type = "grpc".to_string();

        // write with yaml format
        let _file = format!("{}/{}{}", &dir, &"/cluster-resources/cs-", "image.yaml");
        //let serialized_data = serde_yaml::to_string(&itms).unwrap();
        //fs_handler(file,"write",Some(serialized_data)).await?
        Ok(())
    }
}
async fn process_itms_idms(
    dir: String,
    map: HashMap<String, Vec<String>>,
    image_type: String,
    kind: String,
) -> Result<(), MirrorError> {
    let mut vec_mirrors: Vec<MirrorSource> = Vec::new();
    let mut buffered_output = String::new();
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
        write!(buffered_output, "---\n").unwrap();
        let _ = writeln!(buffered_output, "{}", serialized_data);
        let idms_file = format!(
            "{}/{}",
            dir.clone(),
            "cluster-resources/idms-image-mirror.yaml"
        );
        fs_handler(idms_file, "write", Some(buffered_output)).await?;
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
        write!(buffered_output, "---\n").unwrap();
        let _ = writeln!(buffered_output, "{}", serialized_data);
        let itms_file = format!(
            "{}/{}",
            dir.clone(),
            "cluster-resources/itms-image-mirror.yaml"
        );
        fs_handler(itms_file, "write", Some(buffered_output)).await?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use mirror_utils::fs_copy;
    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }
    #[test]
    fn generate_idms_itms_pass() {
        let _ = aw!(fs_handler(
            "test-artifacts/tmp-metadata".to_string(),
            "create_dir",
            None
        ));
        let _ = aw!(fs_copy(
            "test-artifacts/do-not-delete/mirror-metadata/additional-image-reference.json"
                .to_string(),
            "test-artifacts/tmp-metadata/additional-image-reference.json".to_string()
        ));
        let _ = aw!(fs_copy(
            "test-artifacts/do-not-delete/mirror-metadata/operator-image-reference.json"
                .to_string(),
            "test-artifacts/tmp-metadata/operator-image-reference.json".to_string()
        ));
        let _ = aw!(fs_copy(
            "test-artifacts/do-not-delete/mirror-metadata/release-image-reference.json".to_string(),
            "test-artifacts/tmp-metadata/release-image-reference.json".to_string()
        ));

        let log = &Logging {
            log_level: Level::INFO,
        };

        let g_impl = GenerateClusterResources::new("test-artifacts".to_string());
        let res = aw!(g_impl.generate_idms_itms(
            log,
            "test-artifacts".to_string(),
            "docker://localhost:5000/test".to_string(),
        ));
        assert_eq!(res.is_ok(), true)
    }

    #[test]
    fn generate_catalog_source_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };

        let g_impl = GenerateClusterResources::new("test-artifacts".to_string());
        let res = aw!(g_impl.generate_catalog_source(
            log,
            "test-artifacts".to_string(),
            "docker://localhost:5000/test".to_string(),
        ));
        assert_eq!(res.is_ok(), true)
    }

    #[test]
    fn untar_metadata_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };

        let g_impl =
            GenerateClusterResources::new("test-artifacts/do-not-delete/artifacts".to_string());
        let res = aw!(g_impl.untar_metadata(log));
        assert_eq!(res.is_ok(), true);
        let _ = aw!(g_impl.clean_up("test-artifacts".to_string()));
    }

    #[test]
    fn untar_metadata_fail() {
        let log = &Logging {
            log_level: Level::INFO,
        };

        let g_impl = GenerateClusterResources::new("test-artifacts/nada".to_string());
        let res = aw!(g_impl.untar_metadata(log));
        assert_eq!(res.is_err(), true);
        let _ = aw!(g_impl.clean_up("test-artifacts".to_string()));
    }
}
