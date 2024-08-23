use crate::error::handler::MirrorError;
use custom_logger::*;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use mirror_auth::get_token;
use mirror_copy::*;
use std::collections::HashMap;

pub async fn execute_batch(
    log: &Logging,
    dir: String,
    verify_blob: bool,
    map_in: HashMap<String, Vec<FsLayer>>,
) -> Result<(), MirrorError> {
    let mut futs = FuturesUnordered::new();
    let batch_size = 8;
    let bar = "% completed    [--------------------------------------------------------------]"
        .to_string();

    // get blobs in batch of 8
    // each future handles get_blobs api call

    // batch the calls
    for (k, v) in map_in.clone() {
        let hld = k.split("https://").nth(1).unwrap();
        let registry = hld.split("/").nth(0).unwrap();
        log.trace(&format!("url {}", k));
        let token = get_token(log, registry.to_string()).await;
        let mut count = 0;
        let per_position = v.len() as f32 / 61.0;
        if token.is_ok() {
            if v.len() > 0 {
                log.info(&format!("downloading {} blobs", v.len()));
            }
            for layer in v.iter() {
                futs.push(get_blob(
                    log,
                    dir.clone() + &"/blobs-store/",
                    k.clone(),
                    token.as_ref().unwrap().clone(),
                    verify_blob,
                    layer.blob_sum.clone(),
                ));
                if futs.len() >= batch_size {
                    let response = futs.next().await.unwrap();
                    if response.is_err() {
                        let err = MirrorError::new(&format!(
                            "response batch worker {}",
                            response.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
                count += 1;
                if count % 10 == 0 {
                    let update = count as f32 / per_position;
                    let new_bar = bar.replacen("-", "#", update.floor() as usize);
                    log.mid(&new_bar);
                }
            }
        } else {
            let err = MirrorError::new(&format!(
                "token {}",
                token.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    }
    // Wait for the remaining to finish.
    while let Some(response) = futs.next().await {
        if response.is_err() {
            let err = MirrorError::new(&format!(
                "futures reponse {}",
                response.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    }

    for (_k, v) in map_in {
        if v.len() > 0 {
            let new_bar = bar.replacen("-", "#", 62);
            log.mid(&new_bar);
            break;
        }
    }

    Ok(())
}
