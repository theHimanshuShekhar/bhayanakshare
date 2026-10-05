//! The blinded beacon: how a Device is visible nearby to the Devices that hold its Device ID and
//! to nobody else (spec section 3; design D2 of `research/contacts-only-visibility.md`).
//!
//! The Device ID is the only secret. Whoever holds it can work out everything below, and
//! nobody else can:
//!
//! - **epoch**: the Unix time in 10-minute steps.
//! - **label**: the mDNS instance label, the first 16 bytes of BLAKE3 `derive_key(ID || epoch)`
//!   as 32 hex characters. It looks random and changes every epoch, so an onlooker can neither
//!   tie it to a Device ID nor tell that the label of one epoch and the label of the next
//!   belong to the same Device.
//! - **seal**: the Device Name and the real ports, in a TXT attribute sealed with AES-256-GCM.
//!   The key is a second BLAKE3 `derive_key(ID || epoch)` under its own context string, the
//!   nonce is random and travels in front of the ciphertext, and the label is the associated
//!   data, so a sealed value is good only under the label it was announced with. The plaintext
//!   is padded to a fixed size, so every beacon is the same length whatever the name.
//!
//! A listener that holds some Device IDs computes the labels of all of them for the previous,
//! current and next epoch (the window that covers clocks a few minutes apart) and looks each
//! heard label up in that table. Only a hit can be opened, and only with the key of the ID it
//! hit. The announcer sends one beacon however many Devices hold its ID, and the listener does
//! three hashes per ID it holds per epoch: neither depends on the other side's Contacts.
//!
//! What this does not hide: that some Device on this address runs BhayanakShare (the packet is
//! on our service name, from its IP), and, from anyone who holds the ID, that the Device is
//! there. Holding the ID is also enough to forge a beacon, so a name heard in one is as
//! trustworthy as the secrecy of the ID. The SRV port of a beacon is a constant, since
//! `swarm-discovery` cannot announce without one; the real ports are in the seal, because a
//! port that stayed the same from one epoch to the next would link the beacons after all.

use std::collections::HashMap;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};

use crate::{clock::UnixMillis, device_name, identity::DeviceId};

/// How long one label lasts.
pub(crate) const EPOCH_MS: UnixMillis = 10 * 60 * 1000;

/// The TXT attribute carrying the sealed part.
pub(crate) const TXT_KEY: &str = "b";

/// The port every beacon announces in its SRV record. The real ports are sealed.
pub(crate) const DECOY_PORT: u16 = 9;

const LABEL_CONTEXT: &str = "bhayanakshare 2026-10 beacon label";
const SEAL_CONTEXT: &str = "bhayanakshare 2026-10 beacon seal";

const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// The sealed plaintext: version, IPv4 port, IPv6 port, name length, then the name, zero-padded.
/// Sized so the whole TXT attribute (`b=` and the base64 of nonce, ciphertext and tag) is 253
/// bytes, within the 254 an attribute can hold.
const PLAIN_LEN: usize = 160;
const HEADER_LEN: usize = 6;
const MAX_NAME_BYTES: usize = PLAIN_LEN - HEADER_LEN;

/// A point in beacon time.
pub(crate) type Epoch = u64;

pub(crate) fn epoch_of(now: UnixMillis) -> Epoch {
    (now.max(0) / EPOCH_MS) as Epoch
}

/// When the epoch after the one `now` is in begins.
pub(crate) fn next_epoch_starts(now: UnixMillis) -> UnixMillis {
    (epoch_of(now) as UnixMillis + 1) * EPOCH_MS
}

fn derive(context: &str, id: &DeviceId, epoch: Epoch) -> [u8; 32] {
    let mut material = [0u8; 40];
    material[..32].copy_from_slice(id.as_bytes());
    material[32..].copy_from_slice(&epoch.to_be_bytes());
    blake3::derive_key(context, &material)
}

/// The instance label `id` announces under in `epoch`.
pub(crate) fn label(id: &DeviceId, epoch: Epoch) -> String {
    HEXLOWER.encode(&derive(LABEL_CONTEXT, id, epoch)[..16])
}

/// What a beacon carries besides its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Contents {
    /// The Device Name as announced: untrusted text. `None` if it announced none.
    pub name: Option<String>,
    /// The ports of the Device's IPv4 and IPv6 sockets; 0 for none.
    pub v4_port: u16,
    pub v6_port: u16,
}

/// Seals `name` and the ports for `id` in `epoch`: returns the label to announce under and
/// the value of the TXT attribute.
pub(crate) fn seal(
    id: &DeviceId,
    epoch: Epoch,
    name: &str,
    v4_port: u16,
    v6_port: u16,
) -> (String, String) {
    let name = device_name::truncate_bytes(name, MAX_NAME_BYTES);
    let mut plain = [0u8; PLAIN_LEN];
    plain[0] = VERSION;
    plain[1..3].copy_from_slice(&v4_port.to_be_bytes());
    plain[3..5].copy_from_slice(&v6_port.to_be_bytes());
    plain[5] = name.len() as u8; // at most MAX_NAME_BYTES, which is under 256
    plain[HEADER_LEN..HEADER_LEN + name.len()].copy_from_slice(name.as_bytes());

    let label = label(id, epoch);
    let nonce: [u8; NONCE_LEN] = rand::random();
    let sealed = cipher(id, epoch)
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plain, aad: label.as_bytes() })
        .expect("sealing a buffer in memory cannot fail");
    let mut value = nonce.to_vec();
    value.extend_from_slice(&sealed);
    (label, BASE64URL_NOPAD.encode(&value))
}

/// Opens the TXT value heard under `label`, which must be `id`'s label for `epoch`. `None` for
/// anything that does not open: another Device's, damaged, or from a different label.
pub(crate) fn open(id: &DeviceId, epoch: Epoch, label: &str, value: &str) -> Option<Contents> {
    let bytes = BASE64URL_NOPAD.decode(value.as_bytes()).ok()?;
    if bytes.len() != NONCE_LEN + PLAIN_LEN + TAG_LEN {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(NONCE_LEN);
    let plain = cipher(id, epoch)
        .decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad: label.as_bytes() })
        .ok()?;
    if plain[0] != VERSION {
        return None;
    }
    let name_len = usize::from(plain[5]);
    let name = plain.get(HEADER_LEN..HEADER_LEN + name_len)?;
    Some(Contents {
        name: std::str::from_utf8(name).ok().map(str::to_owned),
        v4_port: u16::from_be_bytes([plain[1], plain[2]]),
        v6_port: u16::from_be_bytes([plain[3], plain[4]]),
    })
}

fn cipher(id: &DeviceId, epoch: Epoch) -> Aes256Gcm {
    Aes256Gcm::new(&derive(SEAL_CONTEXT, id, epoch).into())
}

/// The labels a listener accepts: those of the Device IDs it holds, for the epochs around now.
#[derive(Default)]
pub(crate) struct Index {
    labels: HashMap<String, (DeviceId, Epoch)>,
}

impl Index {
    /// The labels of `ids` for the epoch before, of and after `epoch`.
    pub(crate) fn new(ids: impl IntoIterator<Item = DeviceId>, epoch: Epoch) -> Self {
        let mut labels = HashMap::new();
        for id in ids {
            for e in epoch.saturating_sub(1)..=epoch.saturating_add(1) {
                labels.insert(label(&id, e), (id, e));
            }
        }
        Self { labels }
    }

    /// The Device a heard label belongs to, and the epoch it is for.
    pub(crate) fn recognise(&self, label: &str) -> Option<(DeviceId, Epoch)> {
        self.labels.get(label).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> DeviceId {
        DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[n; 32]).public())
    }

    #[test]
    fn a_label_is_32_hex_characters_of_its_own_device_and_epoch() {
        let l = label(&id(1), 7);
        assert_eq!(l.len(), 32);
        assert!(l.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(l, label(&id(1), 7), "the same inputs, the same label");
        assert_ne!(l, label(&id(1), 8), "a new label every epoch");
        assert_ne!(l, label(&id(2), 7), "and one per Device");
    }

    #[test]
    fn labels_show_nothing_of_the_device_id_or_of_the_epochs_before() {
        let device = id(1);
        let text = device.to_string().to_lowercase();
        let labels: Vec<_> = (0..50).map(|e| label(&device, e)).collect();
        for l in &labels {
            assert!(!text.contains(l.as_str()) && !l.contains(&text[..8]));
        }
        // No two epochs share a prefix or suffix of any length worth linking by.
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a[..8], b[..8]);
                assert_ne!(a[24..], b[24..]);
            }
        }
    }

    #[test]
    fn epochs_are_ten_minutes() {
        assert_eq!(epoch_of(0), 0);
        assert_eq!(epoch_of(EPOCH_MS - 1), 0);
        assert_eq!(epoch_of(EPOCH_MS), 1);
        assert_eq!(epoch_of(-5), 0, "a clock before 1970 is not a reason to fail");
        assert_eq!(next_epoch_starts(0), EPOCH_MS);
        assert_eq!(next_epoch_starts(EPOCH_MS - 1), EPOCH_MS);
        assert_eq!(next_epoch_starts(EPOCH_MS), 2 * EPOCH_MS);
    }

    #[test]
    fn a_sealed_beacon_gives_its_name_and_ports_to_the_id_holder() {
        let (l, value) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        assert_eq!(l, label(&id(1), 7));
        assert_eq!(
            open(&id(1), 7, &l, &value),
            Some(Contents { name: Some("Mum's laptop".to_owned()), v4_port: 4000, v6_port: 4001 })
        );
        // An empty name is no name's worth of text, and a missing port is 0.
        let (l, value) = seal(&id(1), 7, "", 0, 4001);
        assert_eq!(
            open(&id(1), 7, &l, &value),
            Some(Contents { name: Some(String::new()), v4_port: 0, v6_port: 4001 })
        );
    }

    #[test]
    fn only_the_right_id_and_epoch_open_it() {
        let (l, value) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        assert_eq!(open(&id(2), 7, &l, &value), None, "another Device's ID");
        assert_eq!(open(&id(1), 8, &l, &value), None, "another epoch");
        // Under another label, even the right key does not open it.
        assert_eq!(open(&id(1), 7, &label(&id(1), 8), &value), None);
        assert_eq!(open(&id(1), 7, &l, ""), None);
        assert_eq!(open(&id(1), 7, &l, "not base64!"), None);
    }

    #[test]
    fn a_damaged_beacon_does_not_open() {
        let (l, value) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        let mut bytes = BASE64URL_NOPAD.decode(value.as_bytes()).unwrap();
        for i in [0, NONCE_LEN, bytes.len() / 2, bytes.len() - 1] {
            bytes[i] ^= 1;
            assert_eq!(open(&id(1), 7, &l, &BASE64URL_NOPAD.encode(&bytes)), None, "byte {i}");
            bytes[i] ^= 1;
        }
        bytes.pop();
        assert_eq!(open(&id(1), 7, &l, &BASE64URL_NOPAD.encode(&bytes)), None, "truncated");
    }

    #[test]
    fn every_beacon_is_the_same_size_and_fits_one_txt_attribute() {
        let long = "🦀".repeat(64);
        let sizes: Vec<_> = ["", "a", "Mum's laptop", long.as_str()]
            .iter()
            .map(|name| TXT_KEY.len() + 1 + seal(&id(1), 7, name, 1, 2).1.len())
            .collect();
        assert!(sizes.iter().all(|s| *s == sizes[0]), "{sizes:?}");
        assert!(sizes[0] <= 254, "{sizes:?}");
    }

    #[test]
    fn a_long_name_is_cut_on_a_character_boundary() {
        let name = "🦀".repeat(64);
        let (l, value) = seal(&id(1), 7, &name, 1, 2);
        let opened = open(&id(1), 7, &l, &value).unwrap().name.unwrap();
        assert!(!opened.is_empty() && name.starts_with(&opened));
        assert!(opened.len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn the_name_is_not_to_be_seen_in_what_is_announced() {
        let (l, value) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        let announced = format!("{l} {value}");
        assert!(!announced.contains("Mum"));
        assert!(!announced.to_lowercase().contains(&id(1).to_string().to_lowercase()));
    }

    #[test]
    fn sealing_twice_gives_different_bytes_under_the_same_label() {
        let (l1, v1) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        let (l2, v2) = seal(&id(1), 7, "Mum's laptop", 4000, 4001);
        assert_eq!(l1, l2);
        assert_ne!(v1, v2, "a fresh nonce each time");
    }

    #[test]
    fn a_listener_recognises_the_labels_of_the_ids_it_holds_around_now() {
        let index = Index::new([id(1), id(2)], 10);
        for e in [9, 10, 11] {
            assert_eq!(index.recognise(&label(&id(1), e)), Some((id(1), e)));
            assert_eq!(index.recognise(&label(&id(2), e)), Some((id(2), e)));
        }
        for e in [8, 12, 0] {
            assert_eq!(index.recognise(&label(&id(1), e)), None, "epoch {e}");
        }
        assert_eq!(index.recognise(&label(&id(3), 10)), None, "an ID it does not hold");
        assert_eq!(index.recognise(&id(1).to_string().to_lowercase()), None);
    }

    #[test]
    fn the_first_epoch_has_no_epoch_before_it() {
        let index = Index::new([id(1)], 0);
        assert_eq!(index.recognise(&label(&id(1), 0)), Some((id(1), 0)));
        assert_eq!(index.recognise(&label(&id(1), 1)), Some((id(1), 1)));
    }
}
