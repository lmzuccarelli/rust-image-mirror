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
    map_in: HashMap<String, Vec<FsLayer>>,
) -> Result<(), MirrorError> {
    let mut futs = FuturesUnordered::new();
    let batch_size = 8;

    // get blobs in batch of 8
    // each future handles get_blobs api call

    // batch the calls
    for (k, v) in map_in {
        let hld = k.split("https://").nth(1).unwrap();
        let registry = hld.split("/").nth(0).unwrap();
        log.trace(&format!("url {}", k));
        let token = get_token(log, registry.to_string()).await;
        let mut count = 0;
        let amount = v.len() as f32 / 51.0;
        let update = 10.0 / amount;
        let mut bar =
            "% completed    [--------------------------------------------------------------]"
                .to_string();
        if token.is_ok() {
            log.info(&format!("downloading {} blobs", v.len()));
            for layer in v.iter() {
                futs.push(get_blob(
                    log,
                    dir.clone() + &"/blobs-store/",
                    k.clone(),
                    token.as_ref().unwrap().clone(),
                    layer.original_ref.as_ref().unwrap().clone(),
                    layer.blob_sum.clone(),
                ));
                if futs.len() >= batch_size {
                    let response = futs.next().await.unwrap();
                    if response.is_ok() {
                        log.debug(&format!("percentage complete"));
                    } else {
                        let err = MirrorError::new(&format!(
                            "response batch worker {}",
                            response.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
                count += 1;
                if count % 10 == 0 {
                    bar = bar.replacen("-", "#", update.round() as usize);
                    log.mid(&bar);
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
        log.debug(&format!("completed rest of batch {:#?}", response.unwrap()));
    }
    Ok(())
}
