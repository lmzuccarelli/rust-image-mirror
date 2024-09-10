// module schema
use clap::{ArgAction, Parser};
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;

/// rust-container-tool cli struct
#[derive(Parser, Debug)]
#[command(name = "rust-image-mirror")]
#[command(author = "Luigi Mario Zuccarelli <luzuccar@redhat.com>")]
#[command(version = "0.3.0")]
#[command(about = "Used to mirror redhat specific release, operator and additional images", long_about = None)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// config file to use
    #[arg(short, long, value_name = "config")]
    pub config: Option<String>,

    /// set the loglevel. Valid arguments are info, debug, trace
    #[arg(value_enum, long, value_name = "loglevel", default_value = "info")]
    pub loglevel: Option<String>,

    /// set the destination. Valid prefix's are docker:// or file://
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
    #[arg(long, value_name = "dry-run", default_value = "false")]
    pub dry_run: bool,

    /// set the skip-manifest-check flag. Valid arguments are none, release, operators, additional,
    /// all
    #[arg(
        value_enum,
        long,
        value_name = "skip-manifest-check",
        default_value = "none"
    )]
    pub skip_manifest_check: Option<String>,

    /// set the architecture types to mirror valid values are arm64,amd64,ppc64le,s390x,all
    /// you can combine them by adding a comma i.e arm64,amd64 as an example
    #[arg(short, long, value_name = "architecture", default_value = "amd64")]
    pub architecture: String,

    /// set the skip-blob-upload flag. This will skip all blob uploads to remote registry (dev mode)
    #[arg(long, value_name = "skip-blob-upload", action=ArgAction::SetFalse)]
    pub skip_blob_upload: bool,

    /// set the verify-blobs flag (will enable/disable sha56 contents with digest.
    #[arg(short, long, value_name = "verify-blobs", action=ArgAction::SetFalse)]
    pub verify_blobs: bool,

    /// set the tls-verify flag (will use http is set to false).
    #[arg(short, long, value_name = "tls-verify", action=ArgAction::SetFalse)]
    pub tls_verify: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MirrorParameters {
    pub architectures: Vec<String>,
    pub destination: String,
    pub dry_run: bool,
    pub dir: String,
    pub from: String,
    pub skip_blob_upload: bool,
    pub skip_manifest_check: String,
    pub tls_verify: bool,
    pub verify_blobs: bool,
    pub generic_override: HashMap<String, String>,
    pub rebuild_catalogs: Option<bool>,
}
