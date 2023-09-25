// module schema

use clap::Parser;
use serde_derive::Deserialize;
use serde_derive::Serialize;

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestList {
    #[serde(rename = "manifests")]
    pub manifests: Vec<Manifest>,

    #[serde(rename = "mediaType")]
    pub media_type: String,

    #[serde(rename = "schemaVersion")]
    pub schema_version: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    #[serde(rename = "digest")]
    pub digest: Option<String>,

    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,

    #[serde(rename = "platform")]
    pub platform: Option<ManifestPlatform>,

    #[serde(rename = "size")]
    pub size: Option<i64>,

    #[serde(rename = "config")]
    pub config: Option<ManifestConfig>,

    #[serde(rename = "layers")]
    pub layers: Option<Vec<Layer>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ManifestPlatform {
    #[serde(rename = "architecture")]
    pub architecture: String,

    #[serde(rename = "os")]
    pub os: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestConfig {
    pub media_type: String,
    pub size: i64,
    pub digest: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub media_type: String,
    pub size: i64,
    pub digest: String,
}

// used only for operator index manifests
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSchema {
    pub tag: Option<String>,
    pub name: Option<String>,
    pub architecture: Option<String>,
    pub schema_version: Option<i64>,
    pub config: Option<ManifestConfig>,
    pub history: Option<Vec<History>>,
    pub fs_layers: Vec<FsLayer>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    #[serde(rename = "v1Compatibility")]
    pub v1compatibility: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsLayer {
    pub blob_sum: String,
    pub original_ref: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub token: String,
    #[serde(rename = "access_token")]
    pub access_token: String,
    #[serde(rename = "expires_in")]
    pub expires_in: i64,
    #[serde(rename = "issued_at")]
    pub issued_at: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub auths: Auths,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Auths {
    #[serde(rename = "cloud.openshift.com")]
    pub cloud_openshift_com: Option<CloudOpenshiftCom>,
    #[serde(rename = "quay.io")]
    pub quay_io: Option<QuayIo>,
    #[serde(rename = "registry.connect.redhat.com")]
    pub registry_connect_redhat_com: Option<RegistryConnectRedhatCom>,
    #[serde(rename = "registry.redhat.io")]
    pub registry_redhat_io: Option<RegistryRedhatIo>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudOpenshiftCom {
    pub auth: String,
    pub email: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuayIo {
    pub auth: String,
    pub email: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryConnectRedhatCom {
    pub auth: String,
    pub email: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRedhatIo {
    pub auth: String,
    pub email: Option<String>,
}

/// rust-container-tool cli struct
#[derive(Parser, Debug)]
#[command(name = "rust-image-mirror")]
#[command(author = "Luigi Mario Zuccarelli <luzuccar@redhat.com>")]
#[command(version = "0.0.1")]
#[command(about = "Used to mirror redhat specific release, operator and additional images", long_about = None)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// config file to use
    #[arg(short, long, value_name = "config", default_value = "")]
    pub config: Option<String>,
}

/// config schema
#[derive(Serialize, Deserialize, Debug)]
pub struct ImageSetConfig {
    #[serde(rename = "kind")]
    pub kind: String,

    #[serde(rename = "apiVersion")]
    pub api_version: String,

    #[serde(rename = "storageConfig")]
    pub storage_config: Option<StorageConfig>,

    #[serde(rename = "mirror")]
    pub mirror: Mirror,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Mirror {
    #[serde(rename = "platform")]
    pub platform: Option<Platform>,

    #[serde(rename = "operators")]
    pub operators: Option<Vec<Operator>>,

    #[serde(rename = "additionalImages")]
    pub additional_images: Option<Vec<Image>>,

    #[serde(rename = "helm")]
    pub helm: Option<Helm>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Image {
    #[serde(rename = "name")]
    name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Helm {}

#[derive(Serialize, Deserialize, Debug)]
pub struct Operator {
    #[serde(rename = "catalog")]
    pub catalog: String,

    #[serde(rename = "packages")]
    pub packages: Option<Vec<Package>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Package {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "channels")]
    pub channels: Option<Vec<Image>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Platform {
    #[serde(rename = "channels")]
    channels: Vec<Channel>,

    #[serde(rename = "graph")]
    graph: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Channel {
    #[serde(rename = "name")]
    name: String,

    #[serde(rename = "type")]
    channel_type: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StorageConfig {
    #[serde(rename = "registry")]
    registry: Registry,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Registry {
    #[serde(rename = "imageURL")]
    image_url: String,

    #[serde(rename = "skipTLS")]
    skip_tls: bool,
}

// used for DeclarativeConfig (catalog.json)

#[derive(Serialize, Deserialize, Debug)]
pub struct Catalog {
    #[serde(rename = "overview")]
    pub overview: serde_json::Value,
}

// DeclarativeConfig this updates the existing dclrcfg
#[derive(Serialize, Deserialize, Debug)]
pub struct DeclarativeEntries {
    #[serde(rename = "entries")]
    pub entries: Option<Vec<ChannelEntry>>,
    pub channel: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeclarativeConfig {
    #[serde(rename = "schema")]
    pub schema: String,

    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "package")]
    pub package: Option<String>,

    #[serde(rename = "relatedImages")]
    pub related_images: Option<Vec<RelatedImage>>,

    #[serde(rename = "defaultChannel")]
    pub default_channel: Option<String>,

    #[serde(rename = "description")]
    pub description: Option<String>,

    #[serde(rename = "entries")]
    pub entries: Option<Vec<ChannelEntry>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RelatedImage {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "image")]
    pub image: String,
}

// ChannelEntry used in the Channel struct
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChannelEntry {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "replaces")]
    pub replaces: Option<String>,

    #[serde(rename = "skips")]
    pub skips: Option<Vec<String>>,

    #[serde(rename = "skipRange")]
    pub skip_range: Option<String>,
}

// Bundle specifies all metadata and data of a bundle object.
// Top-level fields are the source of truth, i.e. not CSV values.
//
// Notes:
//   - Any field slice type field or type containing a slice somewhere
//     where two types/fields are equal if their contents are equal regardless
//     of order must have a `hash:"set"` field tag for bundle comparison.
//   - Any fields that have a `json:"-"` tag must be included in the equality
//     evaluation in bundlesEqual().
#[derive(Serialize, Deserialize, Debug)]
pub struct Bundle {
    #[serde(rename = "schema")]
    pub schema: String,

    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "package")]
    pub package: String,

    #[serde(rename = "image")]
    pub image: String,

    #[serde(rename = "relatedImages")]
    pub related_images: Vec<RelatedImage>,
    // These fields are present so that we can continue serving
    // the GRPC API the way packageserver expects us to in a
    // backwards-compatible way. These are populated from
    // any `olm.bundle.object` properties.
    //
    // These fields will never be persisted in the bundle blob as
    // first class fields.

    //CsvJSON string   `json:"-"`
    //Objects []string `json:"-"`
}

// ImageReference
#[derive(Debug, Clone)]
pub struct ImageReference {
    pub registry: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub packages: Vec<String>,
}
