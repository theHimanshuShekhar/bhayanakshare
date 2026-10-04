//! The control protocol: ALPN `bhayanakshare/ctrl/1`, length-prefixed postcard frames on one
//! bidirectional stream per Transfer. It negotiates a Transfer (Offer, accept, decline); the
//! content itself moves over iroh-blobs afterwards (ADR 0001).
//!
//! Frame layout: `u32` big-endian body length, then the postcard-encoded [`Message`]. The
//! variant order of [`Message`] is the wire format: only ever append.

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

pub const ALPN: &[u8] = b"bhayanakshare/ctrl/1";

/// Bumped whenever this protocol or the pinned iroh-blobs version changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame body accepted from a peer (the spec's 64 MiB Offer limit).
pub const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// First message each way.
    Hello(Hello),
    /// Sender to Receiver: the proposed Transfer.
    Offer(Offer),
    /// Receiver to Sender.
    Accept,
    /// Receiver to Sender. Carries no reason.
    Decline,
    /// Sender to Receiver, once the content is hashed and the Receiver has accepted: the
    /// go-ahead to fetch. This Sender sends it only after `Accept`, having already allowed
    /// that Receiver to fetch; a Receiver must still cope with it arriving before `Accept`.
    HashReady { collection_hash: [u8; 32] },
    /// Receiver to Sender, after the file is verified, in the save folder and fsynced.
    Completed,
    /// Receiver to Sender, while fetching: bytes received so far. For display only.
    Progress { bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub app_version: String,
}

impl Hello {
    pub fn current() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// An Offer of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// Random 128-bit Transfer ID chosen by the Sender.
    pub transfer_id: [u8; 16],
    pub name: String,
    pub size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("connection closed")]
    Closed,
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(u64),
    #[error("malformed frame: {0}")]
    Malformed(#[from] postcard::Error),
    #[error(transparent)]
    Io(std::io::Error),
}

pub async fn write_frame<W>(w: &mut W, msg: &Message) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    let body = postcard::to_stdvec(msg)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME_LEN)
        .ok_or(FrameError::TooLarge(body.len() as u64))?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame).await.map_err(FrameError::Io)
}

/// Not cancel-safe: dropping the future mid-frame loses bytes. Use [`spawn_reader`] to wait
/// on a frame alongside something else.
pub async fn read_frame<R>(r: &mut R) -> Result<Message, FrameError>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len.into()));
    }
    // Read through `take` so a peer that announces a huge frame cannot make us allocate
    // it up front.
    let mut body = Vec::new();
    let read = r.take(len.into()).read_to_end(&mut body).await.map_err(FrameError::Io)?;
    if read as u64 != u64::from(len) {
        return Err(FrameError::Closed);
    }
    Ok(postcard::from_bytes(&body)?)
}

/// Reads frames on a task and forwards them, so callers can `select!` on the receiver
/// without risking a half-read frame. The channel ends after the first error.
pub fn spawn_reader<R>(mut r: R) -> mpsc::Receiver<Result<Message, FrameError>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            let frame = read_frame(&mut r).await;
            let failed = frame.is_err();
            if tx.send(frame).await.is_err() || failed {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_messages() -> Vec<Message> {
        vec![
            Message::Hello(Hello { protocol_version: 1, app_version: "0.1.0".into() }),
            Message::Offer(Offer { transfer_id: [9; 16], name: "photo.jpg".into(), size: 1 << 40 }),
            Message::Accept,
            Message::Decline,
            Message::HashReady { collection_hash: [3; 32] },
            Message::Completed,
            Message::Progress { bytes: 1 << 33 },
        ]
    }

    #[tokio::test]
    async fn every_message_round_trips_through_a_frame() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        for msg in sample_messages() {
            write_frame(&mut a, &msg).await.unwrap();
            assert_eq!(read_frame(&mut b).await.unwrap(), msg);
        }
    }

    #[test]
    fn variant_order_is_the_wire_format() {
        // The first postcard byte is the variant index; reordering variants breaks peers.
        let tags: Vec<u8> = sample_messages()
            .iter()
            .map(|m| postcard::to_stdvec(m).unwrap()[0])
            .collect();
        assert_eq!(tags, [0, 1, 2, 3, 4, 5, 6]);
    }

    #[tokio::test]
    async fn frames_are_length_prefixed_big_endian() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_frame(&mut a, &Message::Accept).await.unwrap();
        drop(a);
        let mut raw = Vec::new();
        b.read_to_end(&mut raw).await.unwrap();
        assert_eq!(raw, [0, 0, 0, 1, 2]);
    }

    #[tokio::test]
    async fn clean_close_before_a_frame_is_closed() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn close_mid_frame_is_closed() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&[0, 0, 0, 10, 1, 2]).await.unwrap();
        drop(a);
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_without_reading_it() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&(MAX_FRAME_LEN + 1).to_be_bytes()).await.unwrap();
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::TooLarge(_))));
    }

    #[tokio::test]
    async fn garbage_body_is_malformed() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&[0, 0, 0, 2, 0xff, 0xff]).await.unwrap();
        assert!(matches!(read_frame(&mut b).await, Err(FrameError::Malformed(_))));
    }

    #[tokio::test]
    async fn reader_task_forwards_frames_then_the_error() {
        let (mut a, b) = tokio::io::duplex(4096);
        let mut rx = spawn_reader(b);
        write_frame(&mut a, &Message::Accept).await.unwrap();
        write_frame(&mut a, &Message::Completed).await.unwrap();
        drop(a);
        assert_eq!(rx.recv().await.unwrap().unwrap(), Message::Accept);
        assert_eq!(rx.recv().await.unwrap().unwrap(), Message::Completed);
        assert!(matches!(rx.recv().await.unwrap(), Err(FrameError::Closed)));
        assert!(rx.recv().await.is_none());
    }
}
