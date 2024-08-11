// module schema
use clap::Parser;
use serde_derive::{Deserialize, Serialize};

/// rust-container-tool cli struct
#[derive(Parser, Debug)]
#[command(name = "rust-image-mirror")]
#[command(author = "Luigi Mario Zuccarelli <luzuccar@redhat.com>")]
#[command(version = "0.2.0")]
#[command(about = "Used to mirror redhat specific release, operator and additional images", long_about = None)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// config file to use
    #[arg(short, long, value_name = "config", default_value = "")]
    pub config: Option<String>,

    /// set the loglevel. Valid arguments are info, debug, trace
    #[arg(value_enum, long, value_name = "loglevel", default_value = "info")]
    pub loglevel: Option<String>,

    /// set the destination. Valid prefix are docker:// or file://
    #[arg(
        short,
        long,
        value_name = "destination",
        default_value = "file://working-dir"
    )]
    pub destination: String,

    /// set the directory to where tar artifacts are stored
    /// used in disk-to-mirror mode
    #[arg(short, long, value_name = "from", default_value = "file://working-dir")]
    pub from: String,

    /// set the dry-run flag.
    /// dont perform a mirror but create a mapping.txt file of all related images
    #[arg(short, long, value_name = "dry-run", default_value = "false")]
    pub dry_run: bool,

    /// set the skip-manifest-check flag. Valid arguments are none, release, operators, additional,
    /// release-operators
    #[arg(
        value_enum,
        long,
        value_name = "skip-manifest-check",
        default_value = "none"
    )]
    pub skip_manifest_check: Option<String>,

    /// set the skip-gen-declconfig flag.
    /// release-operators
    #[arg(
        short,
        long,
        value_name = "skip-gen-declconfig",
        default_value = "false"
    )]
    pub skip_gen_declconfig: bool,

    /// set the skip-blob-upload flag.
    #[arg(short, long, value_name = "skip-blob-upload", default_value = "false")]
    pub skip_blob_upload: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialOrd, PartialEq, Ord, Eq)]
pub struct MirrorImageInfo {
    #[serde(rename = "indexReference")]
    pub reference: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "namespace")]
    pub namespace: String,
    #[serde(rename = "digest")]
    pub digest: String,
    #[serde(rename = "tag")]
    pub tag: Option<String>,
    #[serde(rename = "arch")]
    pub arch: String,
    #[serde(rename = "manifestType")]
    pub manifest_type: String,
    #[serde(rename = "creationTimestamp")]
    pub created: String,
    #[serde(rename = "mirrorType")]
    pub mirror_type: String,
    #[serde(rename = "bundle")]
    pub bundle: Option<String>,
}
