//! The Mainline DHT as a place to publish this Device's address and look up its Contacts'
//! (spec section 3), behind the "public DHT" setting.
//!
//! The service iroh is given is [`PublicDhtLookup`], a wrapper around `DhtAddressLookup` with an
//! enabled flag: while it is off nothing is published and nothing is looked up. The DHT node
//! itself is not even started until the flag is first on and something needs it. Only the relay
//! URL goes into the DHT (`DhtAddressLookup` filters the rest out), and this Device sets no
//! `UserData`, so no Device Name ever goes there.
//!
//! `DhtAddressLookup` keeps republishing for as long as it lives and has no way to stop, so it is
//! told never to republish by itself; the wrapper republishes hourly while it is enabled instead,
//! and switching it off really does stop the DHT writes.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use iroh::{
    EndpointId, SecretKey,
    address_lookup::{AddressLookup, EndpointData, Error as LookupError, Item},
};
use iroh_mainline_address_lookup::DhtAddressLookup;
use n0_future::{boxed::BoxStream, task::AbortOnDropHandle};

use crate::db::Db;

/// The setting that holds whether the public DHT is used: `"1"` or `"0"`. On when unset.
pub(crate) const SETTING: &str = "public_dht";

/// How often the address is published again while the DHT is on. Records in the DHT lapse after
/// about two hours.
const REPUBLISH: Duration = Duration::from_secs(60 * 60);

/// What is passed to `DhtAddressLookup` as its own republish delay: ten years, i.e. never.
const NEVER: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// Whether the public DHT is on, as stored.
pub(crate) async fn load(db: &Db) -> bool {
    match db.setting(SETTING).await {
        Ok(value) => value.as_deref() != Some("0"),
        Err(e) => {
            tracing::warn!("could not read the public DHT setting: {e}");
            true
        }
    }
}

/// The handle a Device keeps on the DHT service it gave to iroh.
#[derive(Clone)]
pub(crate) struct PublicDht {
    state: Arc<State>,
}

struct State {
    enabled: AtomicBool,
    secret: SecretKey,
    /// Built when first needed with the flag on.
    dht: Mutex<Option<DhtAddressLookup>>,
    /// What iroh last asked to publish, to publish again on a timer or when switched on.
    last: Mutex<Option<EndpointData>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicDht").field("enabled", &self.enabled).finish_non_exhaustive()
    }
}

impl PublicDht {
    pub(crate) fn new(secret: SecretKey, enabled: bool) -> Self {
        let state = State {
            enabled: AtomicBool::new(enabled),
            secret,
            dht: Mutex::new(None),
            last: Mutex::new(None),
        };
        Self { state: Arc::new(state) }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.state.enabled.load(Ordering::Acquire)
    }

    /// Switches the DHT on or off. Switching on publishes the address straight away.
    pub(crate) fn set_enabled(&self, on: bool) {
        self.state.enabled.store(on, Ordering::Release);
        if on {
            self.state.publish();
        }
    }

    /// The service to hand to iroh. Must be called inside the Tokio runtime: it starts the timer
    /// that republishes.
    pub(crate) fn lookup(&self) -> PublicDhtLookup {
        let state = self.state.clone();
        let timer = tokio::spawn(async move {
            loop {
                tokio::time::sleep(REPUBLISH).await;
                state.publish();
            }
        });
        PublicDhtLookup { state: self.state.clone(), _timer: AbortOnDropHandle::new(timer) }
    }
}

impl State {
    /// The DHT node, started on first use. `None` if it cannot be started.
    fn dht(&self) -> Option<DhtAddressLookup> {
        let mut dht = self.dht.lock().unwrap_or_else(|e| e.into_inner());
        if dht.is_none() {
            let built = DhtAddressLookup::builder()
                .secret_key(self.secret.clone())
                .republish_delay(NEVER)
                .build();
            match built {
                Ok(built) => *dht = Some(built),
                Err(e) => tracing::warn!("could not start the DHT: {e}"),
            }
        }
        dht.clone()
    }

    /// Publishes the last address iroh gave, if the DHT is on and there is one.
    fn publish(&self) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let (Some(last), Some(dht)) = (last, self.dht()) {
            dht.publish(&last);
        }
    }
}

/// The address lookup service iroh uses for the DHT. Dropping it stops its republish timer.
#[derive(Debug)]
pub(crate) struct PublicDhtLookup {
    state: Arc<State>,
    _timer: AbortOnDropHandle<()>,
}

impl AddressLookup for PublicDhtLookup {
    fn publish(&self, data: &EndpointData) {
        *self.state.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(data.clone());
        self.state.publish();
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
        if !self.state.enabled.load(Ordering::Acquire) {
            return None;
        }
        self.state.dht()?.resolve(endpoint_id)
    }
}

#[cfg(test)]
mod tests {
    use iroh::EndpointAddr;

    use super::*;

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    fn data() -> EndpointData {
        let relay = "https://relay.example./".parse().unwrap();
        EndpointData::from(EndpointAddr::new(secret(2).public()).with_relay_url(relay))
    }

    #[tokio::test]
    async fn a_switched_off_dht_publishes_and_looks_up_nothing_and_is_never_started() {
        let dht = PublicDht::new(secret(1), false);
        let lookup = dht.lookup();
        lookup.publish(&data());
        assert!(lookup.resolve(secret(2).public()).is_none());
        assert!(!dht.enabled());
        // No DHT node, so no socket and no traffic, until it is switched on.
        assert!(dht.state.dht.lock().unwrap().is_none());
        // What iroh published is kept, to publish when it is switched on.
        assert!(dht.state.last.lock().unwrap().is_some());
    }

    /// Needs the public internet: opt in with `BHAYANAKSHARE_TEST_DHT=1`. One Device publishes
    /// through the wrapper (its relay URL, nothing else); another, with no other source of
    /// addresses, finds it by its ID alone.
    #[tokio::test]
    async fn the_dht_resolves_a_published_relay_url() {
        use std::time::Duration;

        if std::env::var_os("BHAYANAKSHARE_TEST_DHT").is_none() {
            eprintln!("skipped: set BHAYANAKSHARE_TEST_DHT=1 to test against the public DHT");
            return;
        }
        // A key of its own, so that nothing else on the DHT is under the ID looked up.
        let key = SecretKey::from_bytes(&rand::random());
        let published = PublicDht::new(key.clone(), true).lookup();
        let relay = "https://relay.example./".parse().unwrap();
        let addr = EndpointAddr::new(key.public()).with_relay_url(relay);
        published.publish(&EndpointData::from(addr.clone()));

        let finder = PublicDht::new(secret(12), true).lookup();
        let found = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if let Some(mut stream) = finder.resolve(key.public()) {
                    if let Some(Ok(item)) = n0_future::StreamExt::next(&mut stream).await {
                        return item;
                    }
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        })
        .await
        .expect("the DHT did not return the record in 2 minutes");
        assert_eq!(found.relay_urls().collect::<Vec<_>>(), addr.relay_urls().collect::<Vec<_>>());
        assert!(found.user_data().is_none(), "a record on the DHT carries no UserData");
    }
}
