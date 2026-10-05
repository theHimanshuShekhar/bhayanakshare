//! The control protocol: ALPN `bhayanakshare/ctrl/1`, length-prefixed postcard frames on one
//! bidirectional stream per Transfer. It negotiates a Transfer (Offer, accept, decline, busy,
//! cancel, expiry) and picks it up again after a lost connection (resume); the content
//! itself moves over iroh-blobs afterwards (ADR 0001).
//!
//! Frame layout: `u32` big-endian body length, then the postcard-encoded [`Message`]. The
//! variant order of [`Message`] is the wire format: only ever append.

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

use crate::manifest::{Manifest, ManifestError};

pub const ALPN: &[u8] = b"bhayanakshare/ctrl/1";

/// Bumped whenever this protocol or the pinned iroh-blobs version changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 3;

/// Largest frame body accepted from a peer (the spec's 64 MiB Offer limit).
pub const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

/// The most text, in bytes of UTF-8, an Offer carries inline (spec section 5). Longer text is
/// sent as a file named [`LONG_TEXT_NAME`].
pub const MAX_INLINE_TEXT: usize = 64 * 1024;

/// What a text too long to go inline is called as a file.
pub const LONG_TEXT_NAME: &str = "text.txt";

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
    /// Receiver to Sender, in reply to an Offer: this Receiver already has 5 Offers from this
    /// Sender waiting for an answer. The Offer is dropped, not queued.
    Busy,
    /// Either side, any time before `Completed`: stop. The Receiver deletes what it has.
    Cancel,
    /// Either side, while the Offer is unanswered: the sender of this message has timed it
    /// out (10 minutes by its own clock). Both sides time an Offer, so without this the one
    /// whose clock fires first would just hang up and the other would report a lost
    /// connection. The spec's table has no such message.
    Expired,
    /// Receiver to Sender, first message on a new connection: the control connection of an
    /// accepted Transfer was lost, so the Receiver redialled. The Receiver drives resume.
    Resume { transfer_id: [u8; 16] },
    /// Sender to Receiver, answering `Resume`: the Transfer is still running here and its
    /// files are as offered. The Sender has allowed this Receiver to fetch again (a fresh
    /// grant) before sending it, so it is the go-ahead; the Receiver already has the content
    /// hash from `HashReady`.
    ResumeOk,
    /// Sender to Receiver, answering `Resume`: this Device has no such Transfer for you.
    Unknown,
    /// Sender to Receiver: the Transfer failed on the Sender, for this plain-language reason.
    /// Answers `Resume`, or comes instead of `HashReady` when a file changed or went missing
    /// before the Sender could serve it. (A Sender that cancelled it answers `Cancel`.)
    Failed { reason: String },
    /// Receiver to Sender, in reply to an Offer: its manifest is malformed or over a limit
    /// (spec sections 5 and 6). The Offer is dropped without being shown to anyone, and the
    /// Sender shows "Couldn't be sent: invalid file names". The spec's table has no such
    /// message.
    InvalidOffer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub app_version: String,
    /// The sender's Device Name; empty if it has none to show. Untrusted text.
    pub device_name: String,
}

impl Hello {
    /// A Hello for this build with no Device Name, as a hand-written peer sends it.
    pub fn current() -> Self {
        Self::named(String::new())
    }

    pub fn named(device_name: String) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            device_name,
        }
    }
}

/// What an Offer proposes to send: the kind of Transfer, with its content when that is small
/// enough to travel in the Offer itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferKind {
    /// Files and folders. Their content is fetched after the Receiver accepts.
    Files(Manifest),
    /// A piece of text of at most [`MAX_INLINE_TEXT`] bytes, which is the whole content: there
    /// is nothing to hash or fetch. Untrusted: show it as plain text only.
    Text(String),
}

/// The Sender's proposal of a Transfer: what it would send, which is all the Receiver needs
/// to decide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// Random 128-bit Transfer ID chosen by the Sender.
    pub transfer_id: [u8; 16],
    /// The total size of the files, or the length of the text, in bytes.
    pub size: u64,
    /// How many files there are; none for text.
    pub file_count: u64,
    /// Symlinks found in the chosen folders and left out.
    pub skipped_links: u32,
    pub kind: OfferKind,
}

impl Offer {
    /// An Offer of what `manifest` lists, with the totals worked out from it.
    pub fn new(transfer_id: [u8; 16], manifest: Manifest, skipped_links: u32) -> Self {
        Self {
            transfer_id,
            size: manifest.total_size().unwrap_or(u64::MAX),
            file_count: manifest.file_count(),
            skipped_links,
            kind: OfferKind::Files(manifest),
        }
    }

    /// An Offer of `text`, sent inline.
    pub fn text(transfer_id: [u8; 16], text: String) -> Self {
        Self { transfer_id, size: text.len() as u64, file_count: 0, skipped_links: 0, kind: OfferKind::Text(text) }
    }

    /// Checks the Offer as the Receiver must, before anyone sees it: the manifest is valid
    /// (or the text is within its limit) and the totals the Offer states are what it adds up
    /// to.
    pub fn validate(&self) -> Result<(), ManifestError> {
        match &self.kind {
            OfferKind::Files(manifest) => {
                manifest.validate()?;
                if manifest.total_size() != Some(self.size) || manifest.file_count() != self.file_count {
                    return Err(ManifestError::Inconsistent);
                }
            }
            OfferKind::Text(text) => {
                if text.is_empty() {
                    return Err(ManifestError::Empty);
                }
                if text.len() > MAX_INLINE_TEXT {
                    return Err(ManifestError::TextTooLong);
                }
                if self.size != text.len() as u64 || self.file_count != 0 || self.skipped_links != 0 {
                    return Err(ManifestError::Inconsistent);
                }
            }
        }
        Ok(())
    }
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
    use crate::manifest::Entry;

    fn sample_messages() -> Vec<Message> {
        vec![
            Message::Hello(Hello {
                protocol_version: 1,
                app_version: "0.1.0".into(),
                device_name: "Mum's laptop".into(),
            }),
            Message::Offer(Offer::new(
                [9; 16],
                Manifest {
                    entries: vec![
                        Entry::File { path: "photos/a.jpg".into(), size: 1 << 40, mtime_ns: 5, executable: true },
                        Entry::empty_dir("photos/empty"),
                    ],
                },
                2,
            )),
            Message::Accept,
            Message::Decline,
            Message::HashReady { collection_hash: [3; 32] },
            Message::Completed,
            Message::Progress { bytes: 1 << 33 },
            Message::Busy,
            Message::Cancel,
            Message::Expired,
            Message::Resume { transfer_id: [9; 16] },
            Message::ResumeOk,
            Message::Unknown,
            Message::Failed { reason: "The other Device went away.".into() },
            Message::InvalidOffer,
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
        assert_eq!(tags, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
    }

    #[test]
    fn an_offer_must_add_up() {
        let manifest = Manifest { entries: vec![Entry::file("a", 3), Entry::file("b/c", 4), Entry::empty_dir("d")] };
        let offer = Offer::new([1; 16], manifest, 0);
        assert_eq!((offer.size, offer.file_count), (7, 2));
        assert_eq!(offer.validate(), Ok(()));

        for wrong in [
            Offer { size: 8, ..offer.clone() },
            Offer { size: 6, ..offer.clone() },
            Offer { file_count: 3, ..offer.clone() },
            Offer { file_count: 0, ..offer.clone() },
        ] {
            assert_eq!(wrong.validate(), Err(ManifestError::Inconsistent));
        }
        // What is wrong with the manifest itself is reported first.
        let bad = Offer::new([1; 16], Manifest { entries: vec![Entry::file("../x", 1)] }, 0);
        assert_eq!(bad.validate(), Err(ManifestError::InvalidName(0)));
        // Sizes that overflow cannot match any total.
        let huge = Offer::new([1; 16], Manifest { entries: vec![Entry::file("a", u64::MAX), Entry::file("b", 1)] }, 0);
        assert_eq!(huge.validate(), Err(ManifestError::Inconsistent));
    }

    #[test]
    fn a_text_offer_carries_its_text_and_adds_up() {
        let offer = Offer::text([4; 16], "héllo\nworld".into());
        assert_eq!((offer.size, offer.file_count, offer.skipped_links), (12, 0, 0));
        assert_eq!(offer.validate(), Ok(()));
        // It survives the wire, with the kind and the text intact.
        let bytes = postcard::to_stdvec(&Message::Offer(offer.clone())).unwrap();
        assert_eq!(postcard::from_bytes::<Message>(&bytes).unwrap(), Message::Offer(offer.clone()));

        for wrong in [
            Offer { size: 11, ..offer.clone() },
            Offer { file_count: 1, ..offer.clone() },
            Offer { skipped_links: 1, ..offer.clone() },
        ] {
            assert_eq!(wrong.validate(), Err(ManifestError::Inconsistent));
        }
        assert_eq!(Offer::text([4; 16], String::new()).validate(), Err(ManifestError::Empty));
    }

    #[test]
    fn inline_text_may_be_exactly_64_kib_of_bytes_and_no_more() {
        assert_eq!(Offer::text([1; 16], "a".repeat(MAX_INLINE_TEXT)).validate(), Ok(()));
        let over = Offer::text([1; 16], "a".repeat(MAX_INLINE_TEXT + 1));
        assert_eq!(over.validate(), Err(ManifestError::TextTooLong));
        // The limit counts bytes: 32,768 two-byte characters fit, one more does not.
        assert_eq!(Offer::text([1; 16], "é".repeat(MAX_INLINE_TEXT / 2)).validate(), Ok(()));
        let over = Offer::text([1; 16], "é".repeat(MAX_INLINE_TEXT / 2 + 1));
        assert_eq!(over.validate(), Err(ManifestError::TextTooLong));
    }

    #[test]
    fn text_that_is_not_utf8_does_not_decode() {
        // `Message::Offer`, the ids and totals, then a `Text` kind whose one byte is not UTF-8.
        let mut bytes = postcard::to_stdvec(&Message::Offer(Offer::text([0; 16], "a".into()))).unwrap();
        *bytes.last_mut().unwrap() = 0xff;
        assert!(postcard::from_bytes::<Message>(&bytes).is_err());
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
