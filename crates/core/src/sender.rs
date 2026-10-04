//! The Sender side of a Transfer.
//!
//! Dial the Receiver, say Hello, send the Offer straight away, and hash the file while the
//! Receiver decides (the file is imported by reference, so nothing is copied). Once hashing
//! is done and the Receiver has accepted, allow that Receiver to fetch (see `gate`) and send
//! `HashReady`; the Receiver then pulls the content over iroh-blobs from this Device's global
//! store. The Transfer ends with the Receiver's `Decline` or `Completed`.

use std::{path::PathBuf, sync::Arc};

use iroh_blobs::{
    BlobFormat, Hash,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    format::collection::Collection,
};
use crate::{
    device::{DeviceAddr, Shared, TransferInfo},
    protocol::{self, Message, Offer, write_frame},
    session::{Failure, LOST, UNEXPECTED, expect_hello, fail},
    transfer::TransferState,
};

pub(crate) async fn run(sh: Arc<Shared>, info: TransferInfo, to: DeviceAddr, path: PathBuf) {
    let outcome = tokio::select! {
        () = sh.cancel.cancelled() => return,
        outcome = flow(&sh, &info, to, path) => outcome,
    };
    if let Err(Failure(reason)) = outcome {
        sh.transition(&info, TransferState::Failed { reason }).await;
    }
}

async fn flow(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    to: DeviceAddr,
    path: PathBuf,
) -> Result<(), Failure> {
    let conn = sh
        .endpoint
        .connect(to.to_endpoint_addr(), protocol::ALPN)
        .await
        .map_err(fail("Could not reach the receiving Device."))?;
    let (mut send, recv) = conn
        .open_bi()
        .await
        .map_err(fail("Could not reach the receiving Device."))?;
    let mut incoming = protocol::spawn_reader(recv);

    write_frame(&mut send, &Message::Hello(protocol::Hello::current()))
        .await
        .map_err(fail(LOST))?;
    expect_hello(&mut incoming).await?;
    write_frame(
        &mut send,
        &Message::Offer(Offer {
            transfer_id: *info.id.as_bytes(),
            name: info.name.clone(),
            size: info.size,
        }),
    )
    .await
    .map_err(fail(LOST))?;

    // Hash while the Receiver decides. Not spawned: leaving this function drops it, which
    // abandons the hashing of a declined or failed Transfer.
    let mut import = Some(Box::pin(import(sh.blobs.clone(), path, info.name.clone())));
    // The temp tags keep the imported blobs alive for as long as the Transfer runs.
    let mut _keep_alive = Vec::new();
    let mut accepted = false;
    let mut root = None;
    let mut hash_sent = false;
    // Lets the Receiver fetch once it has accepted and the content is hashed. Released when
    // the Transfer completes, and on every other way out of this function.
    let mut grant = None;
    let peer = to.id.endpoint_id();

    loop {
        // `HashReady` is sent only once the Receiver has accepted, and only after the grant is
        // taken. Sent earlier, a Receiver holding the hash could dial the provider the moment
        // it says yes, before this loop has read that yes, and be turned away.
        if accepted && !hash_sent {
            if let Some(root) = root {
                grant = Some(sh.gate.allow(peer, root));
                write_frame(&mut send, &Message::HashReady { collection_hash: *root.as_bytes() })
                    .await
                    .map_err(fail(LOST))?;
                hash_sent = true;
            }
        }
        tokio::select! {
            msg = incoming.recv() => match msg {
                Some(Ok(Message::Accept)) if !accepted => {
                    accepted = true;
                    sh.transition(info, TransferState::Accepted).await;
                }
                Some(Ok(Message::Decline)) if !accepted => {
                    sh.transition(info, TransferState::Declined).await;
                    break;
                }
                Some(Ok(Message::Progress { bytes })) if accepted && hash_sent => {
                    sh.progress(info, bytes);
                }
                Some(Ok(Message::Completed)) if accepted && hash_sent => {
                    // Nothing may be fetched once the Transfer is reported finished.
                    drop(grant.take());
                    sh.transition(info, TransferState::Completed { saved_to: None }).await;
                    break;
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "out-of-order message")),
                Some(Err(e)) => return Err(Failure::with(LOST, e)),
                None => return Err(Failure::with(LOST, "stream ended")),
            },
            done = async { import.as_mut().expect("guarded by the if below").await },
                if import.is_some() =>
            {
                import = None;
                let (hash, tags) = done?;
                _keep_alive = tags;
                root = Some(hash);
            }
        }
    }

    // Tell the Receiver nothing more is coming, then close; the Receiver waits for this
    // before dropping its side, so its last frame is never cut off.
    let _ = send.finish();
    conn.close(0u32.into(), b"done");
    Ok(())
}

/// Imports the file by reference into the global store and wraps it in a one-entry
/// Collection, the shape the Receiver fetches (folders will add entries).
async fn import(
    store: iroh_blobs::api::Store,
    path: PathBuf,
    name: String,
) -> Result<(Hash, Vec<TempTag>), Failure> {
    let file = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path,
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .map_err(fail("Could not read the file."))?;
    let collection = Collection::from_iter([(name, file.hash())]);
    let root = collection
        .store(&store)
        .await
        .map_err(fail("Could not prepare the file for sending."))?;
    Ok((root.hash(), vec![file, root]))
}
