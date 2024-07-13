use custom_logger::*;
use serde_derive::{Deserialize, Serialize};

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

impl ImageDigestMirrorSet {
    fn new(&mut self) {
        self.api_version = "operators.coreos.com/v1alpha1".to_string();
        self.kind = "ImageDigestMirrorsSet".to_string();
    }
    fn generate_idms(&self, log: &Logging, dir: String) -> Result<(), Box<dyn std::error::Error>> {
        log.info(&format!("hello world {}", dir));
        Ok(())
    }
}

impl ImageTagMirrorSet {
    fn new(&mut self) {
        self.api_version = "config.openshift.io/v1".to_string();
    }
    fn generate_itms(&self, log: &Logging, dir: String) -> Result<(), Box<dyn std::error::Error>> {
        log.info(&format!("hello world {}", dir));
        Ok(())
    }
}

impl CatalogSource {
    fn new(&mut self) {
        self.api_version = "config.openshift.io/v1".to_string();
        self.kind = "CatalogSource".to_string();
        self.metadata.namespace = Some("openshift-marketplace".to_string());
        self.spec.source_type = "grpc".to_string();
    }
    fn generate_catalog_source(
        &mut self,
        log: &Logging,
        dir: String,
        catalog: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        log.info("generating catalogsourcerld");
        self.spec.image = catalog.replace(":", "-").replace(".", "-").to_string();
        // write with yaml format
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(format!(
                "{}/{}{}{}",
                &dir, &"cluster-resources/cs-", self.spec.image, ".yaml"
            ))
            .expect("Couldn't open file");
        serde_yaml::to_writer(file, &self).unwrap();
        Ok(())
    }
}
