use crate::mirror::utils::fs_handler;
use crate::podman::process::*;
use async_trait::async_trait;
use custom_logger::*;
use flate2::read::GzDecoder;
use mirror_error::MirrorError;
use reqwest::Client;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use tar::Archive;

#[derive(Debug, Clone)]
pub struct ImplGraphDataInterface {}

#[allow(unused)]
#[async_trait]
pub trait GraphDataInterface {
    // used to interact with cincinnati api
    async fn get_graphdata(&self, url: String) -> Result<String, Box<dyn std::error::Error>>;
    async fn get_graph_tar_gz(
        &self,
        url: String,
        file_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>>;
    async fn build_graph_image(&self, log: &Logging, dir: String) -> Result<(), MirrorError>;
    async fn build_image_cleanup(&self);
}

#[async_trait]
impl GraphDataInterface for ImplGraphDataInterface {
    async fn get_graphdata(&self, url: String) -> Result<String, Box<dyn std::error::Error>> {
        let client = Client::new();
        // check without token
        let body = client
            .get(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await?
            .text()
            .await?;

        Ok(body)
    }

    async fn get_graph_tar_gz(
        &self,
        url: String,
        file_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response = reqwest::get(url).await?;
        let mut file = std::fs::File::create(file_name)?;
        let mut content = Cursor::new(response.bytes().await?);
        std::io::copy(&mut content, &mut file)?;
        Ok(())
    }

    async fn build_graph_image(&self, log: &Logging, dir: String) -> Result<(), MirrorError> {
        let url = "https://api.openshift.com/api/upgrades_info/graph-data".to_string();
        let tar_gz_file = format!("{}/artifacts/cincinnati-graph-data.tar.gz", dir);
        let exists = Path::new(&tar_gz_file).exists();
        if !exists {
            let graph_res = self.get_graph_tar_gz(url.clone(), &tar_gz_file).await;
            if graph_res.is_err() {
                let err = MirrorError::new(&format!(
                    "[build-graph-image] api call to graph data tar.gz {} {}",
                    tar_gz_file,
                    graph_res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }
        fs_handler("container".to_string(), "create_dir", None).await?;
        let data = std::fs::File::open(&tar_gz_file);
        if data.is_ok() {
            let gz = GzDecoder::new(data.unwrap());
            let mut archive = Archive::new(gz);
            let res_archive = archive.unpack("container/");
            if res_archive.is_ok() {
                fs::copy("hack/graph-data.containerfile", "graph-data.containerfile")
                    .expect("should copy containerfile");
                // build the graph image
                let res_build = build(
                    log,
                    "openshift/graph-image:latest".to_string(),
                    "graph-data.containerfile".to_string(),
                );
                if res_build.is_err() {
                    let err = MirrorError::new(&format!(
                        "[build-graph-image] building graph image {}",
                        res_build.err().unwrap().to_string().to_lowercase()
                    ));
                    return Err(err);
                } else {
                    let res_save = save(
                        log,
                        "openshift/graph-image:latest".to_string(),
                        "graph-image".to_string(),
                    );
                    if res_save.is_err() {
                        //self.cleanup();
                        let err = MirrorError::new(&format!(
                            "[build-graph-image] saving graph image {}",
                            res_save.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
            } else {
                let err = MirrorError::new(&format!(
                    "[build-graph-image] unpacking cincinnati-graph-data tar.gz {}",
                    res_archive.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            let err = MirrorError::new(&format!(
                "[build-graph-image] cincinnati-graph-data tar.gz {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    async fn build_image_cleanup(&self) {
        if Path::new("graph-data.containerfile").exists() {
            fs::remove_file("graph-data.containerfile").expect("delete containerfile");
        }
        if Path::new("container").exists() {
            fs::remove_dir_all("container").expect("should delete container directory");
        }
        if Path::new("graph-image").exists() {
            fs::remove_dir_all("graph-image").expect("delete graph-image directory");
        }
    }
}
