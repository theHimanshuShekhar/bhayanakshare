//! Contacts: Devices whose Device ID this Device has saved (spec section 2). The relationship
//! is one-sided and lives only in this Device's database.

use std::net::SocketAddr;

use serde::Serialize;

use crate::{clock::UnixMillis, identity::DeviceId};

/// Longest Nickname or Device Name kept for a Contact, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// A saved Device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Contact {
    pub id: DeviceId,
    /// The name this Device gave the Contact; shown instead of the Device Name.
    pub nickname: Option<String>,
    /// The Contact's own name for itself, as last learned (a share link suggests one).
    pub device_name: Option<String>,
    /// Accept this Contact's Offers without asking. Off by default.
    pub auto_accept: bool,
    pub last_known_address: KnownAddress,
    pub added_at: UnixMillis,
}

impl Contact {
    /// What to call the Contact: the Nickname if there is one, else the Device Name. `None`
    /// when neither is known, and the caller shows the Fingerprint.
    pub fn display_name(&self) -> Option<&str> {
        self.nickname.as_deref().or(self.device_name.as_deref())
    }
}

/// Where a Contact was last reached: its relay and the direct addresses seen on the latest
/// successful connection. Empty until the first one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct KnownAddress {
    pub relay_url: Option<String>,
    pub direct: Vec<SocketAddr>,
}

impl KnownAddress {
    /// What a new connection says replaces what was known. A part the connection did not show
    /// (a LAN connection uses no relay, a relayed one no direct address) is kept, since an old
    /// hint beats none.
    pub(crate) fn updated_with(&self, seen: &KnownAddress) -> KnownAddress {
        KnownAddress {
            relay_url: seen.relay_url.clone().or_else(|| self.relay_url.clone()),
            direct: if seen.direct.is_empty() { self.direct.clone() } else { seen.direct.clone() },
        }
    }

    /// Where to dial `id` from this, or `None` while nothing is known (or what is known cannot be
    /// read back).
    pub(crate) fn to_endpoint_addr(&self, id: DeviceId) -> Option<iroh::EndpointAddr> {
        let relay = self.relay_url.as_deref().and_then(|url| url.parse::<iroh::RelayUrl>().ok());
        let addrs = relay
            .map(iroh::TransportAddr::Relay)
            .into_iter()
            .chain(self.direct.iter().copied().map(iroh::TransportAddr::Ip))
            .collect::<Vec<_>>();
        (!addrs.is_empty()).then(|| iroh::EndpointAddr::from_parts(id.endpoint_id(), addrs))
    }

    /// The paths an established connection is using.
    pub(crate) fn of_connection(conn: &iroh::endpoint::Connection) -> KnownAddress {
        let mut seen = KnownAddress::default();
        for path in conn.paths().iter() {
            match path.remote_addr() {
                iroh::TransportAddr::Relay(url) => {
                    seen.relay_url.get_or_insert_with(|| url.to_string());
                }
                iroh::TransportAddr::Ip(addr) => seen.direct.push(*addr),
                _ => {}
            }
        }
        seen
    }
}

/// Trims a name typed by the user or read from a share link. Empty means "no name".
pub(crate) fn clean_name(name: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if name.chars().count() > MAX_NAME_CHARS {
        return Err("A name can be at most 64 characters.");
    }
    if name.chars().any(char::is_control) {
        return Err("A name cannot contain control characters.");
    }
    Ok(Some(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_empty_means_none() {
        assert_eq!(clean_name(None), Ok(None));
        assert_eq!(clean_name(Some("  \t ")), Ok(None));
        assert_eq!(clean_name(Some("  Alice's laptop ")), Ok(Some("Alice's laptop".to_owned())));
    }

    #[test]
    fn names_are_limited_and_plain() {
        assert!(clean_name(Some(&"x".repeat(64))).is_ok());
        assert!(clean_name(Some(&"x".repeat(65))).is_err());
        // Characters, not bytes.
        assert!(clean_name(Some(&"é".repeat(64))).is_ok());
        assert!(clean_name(Some("a\nb")).is_err());
    }

    #[test]
    fn a_nickname_takes_priority_over_the_device_name() {
        let id = DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[5; 32]).public());
        let mut contact = Contact {
            id,
            nickname: None,
            device_name: None,
            auto_accept: false,
            last_known_address: KnownAddress::default(),
            added_at: 0,
        };
        assert_eq!(contact.display_name(), None);
        contact.device_name = Some("DESKTOP-1".into());
        assert_eq!(contact.display_name(), Some("DESKTOP-1"));
        contact.nickname = Some("Mum".into());
        assert_eq!(contact.display_name(), Some("Mum"));
    }

    #[test]
    fn a_known_address_is_dialled_by_its_relay_and_direct_addresses() {
        let id = DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[6; 32]).public());
        assert_eq!(KnownAddress::default().to_endpoint_addr(id), None);

        let wan: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        let known = KnownAddress { relay_url: Some("https://relay.example./".into()), direct: vec![wan] };
        let addr = known.to_endpoint_addr(id).unwrap();
        assert_eq!(addr.id, id.endpoint_id());
        assert_eq!(addr.relay_urls().map(|url| url.to_string()).collect::<Vec<_>>(), ["https://relay.example./"]);
        assert_eq!(addr.ip_addrs().copied().collect::<Vec<_>>(), [wan]);

        // A relay URL that no longer parses is dropped, not fatal.
        let broken = KnownAddress { relay_url: Some("not a url".into()), direct: vec![] };
        assert_eq!(broken.to_endpoint_addr(id), None);
    }

    #[test]
    fn a_new_connection_replaces_only_what_it_shows() {
        let lan: SocketAddr = "192.168.1.5:4000".parse().unwrap();
        let wan: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        let known = KnownAddress { relay_url: Some("https://relay.example/".into()), direct: vec![lan] };

        let lan_only = KnownAddress { relay_url: None, direct: vec![wan] };
        assert_eq!(
            known.updated_with(&lan_only),
            KnownAddress { relay_url: Some("https://relay.example/".into()), direct: vec![wan] }
        );
        let relay_only = KnownAddress { relay_url: Some("https://other.example/".into()), direct: vec![] };
        assert_eq!(
            known.updated_with(&relay_only),
            KnownAddress { relay_url: Some("https://other.example/".into()), direct: vec![lan] }
        );
        assert_eq!(known.updated_with(&KnownAddress::default()), known);
    }
}
