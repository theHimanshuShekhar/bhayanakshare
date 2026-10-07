# What BhayanakShare depends on from n0's infrastructure

Research for issue #5. Researched 2026-10-03 against **iroh v1.3.0** (tag commit `0072d7d`, released 2026-09-28) and the live docs at docs.iroh.computer.

Legend: **[V]** verified against a primary source (cited). **[I]** inference from verified facts, not stated by n0.

## Short answer

- A rate-limited relay **throttles; it does not disconnect**. The relay stops reading the Sender's socket until its token bucket refills, so data is delayed, never dropped. The client receives a one-time `Status::RateLimited` frame. iroh turns that into a `warn!` log line and a metrics counter. **There is no event or `RelayStatus` field for it.** Throttling only affects traffic that goes through the relay. Once a direct path exists, the limit no longer applies.
- **Dialing by bare EndpointId needs *some* address lookup.** With `presets::N0`, that lookup is n0's pkarr/DNS server at `dns.iroh.link`. The Receiver must publish to it and the Sender must resolve from it. If neither side has a lookup service or any stored address, `connect` fails with `NoAddress`. Alternatives: Mainline DHT (`iroh-mainline-address-lookup` 0.6.0), mDNS on the LAN, or storing a Contact's last-known `EndpointAddr` (relay URL) and feeding it through `MemoryLookup` or a ticket or link.
- **Terms:** n0 publishes no dedicated terms of use for the public relays or iroh.link DNS. They publish a support policy ("development and hobby use only", no SLA, rate-limited with undisclosed limits that can change, abuse monitoring with IP blocking, only the latest iroh major is supported, and old versions can be dropped at any time). There is also a short generic Iroh Services ToS.
- **Configurability:** relay URLs can change later without breaking anything. A Device can dial any relay URL it learns, not only the ones in its own relay map. DNS/pkarr is the part that can break compatibility, because two Devices must share at least one publish/resolve server. Keep publishing to and resolving from `dns.iroh.link` alongside any new server, and design that in from day one.

## 1. What `presets::N0` actually depends on [V]

From `iroh/src/endpoint/presets.rs` (`impl Preset for N0`) and `iroh/src/defaults.rs`:

| Dependency | Endpoint | Used for |
|---|---|---|
| Public relays | `use1-1`, `usw1-1`, `euc1-1`, `aps1-1` `.relay.n0.iroh.link` (HTTPS/WebSocket on TCP 443) | Home relay, NAT-traversal signalling, encrypted fallback transport |
| Relay QUIC address discovery | same hosts, UDP 7842 | Learning own public address (STUN-like) |
| pkarr publisher | `https://dns.iroh.link/pkarr` (HTTP PUT) | Publishing own home-relay URL under the EndpointId |
| pkarr resolver | `https://dns.iroh.link/pkarr` (HTTP GET) | Resolving a Contact's EndpointId |
| DNS lookup | `_iroh.<z32-id>.dns.iroh.link TXT` via system resolver | Resolving a Contact's EndpointId |

- By default the publisher only publishes the **relay URL**, not IP addresses (`AddrFilter::relay_only()`; `pkarr.rs`). Republish happens every 5 min, with TTL 30 s.
- `dns.iroh.link` "does not interact with the Mainline DHT, so is a more central service" (doc comment on `N0_DNS_PKARR_RELAY_PROD`, `pkarr.rs`). The `iroh-dns-server` code can *read* from the DHT as a fallback (`[mainline] enabled`), but it does not publish to it. Whether n0's production instance enables that fallback is not published.
- Gotcha: if the env var `IROH_FORCE_STAGING_RELAYS` is non-empty, `presets::N0` silently switches **both** relays and DNS to n0 staging (`default_relay_mode()`, `DnsAddressLookup::n0_dns()`, `PkarrResolver::n0_dns()`). A Device launched with that variable cannot find production Devices by ID. [V for the switch; I for the consequence]
- Not n0: the DNS resolver's fallback nameservers are public resolvers such as Cloudflare (`iroh-dns/src/dns.rs`), and they are configurable since 1.2.0.

## 2. What happens to a running Transfer when the public relay rate-limits it

### Verified mechanism

- **Server side** (`iroh-relay/src/server/streams.rs`, `RateLimited<S>::poll_read`): each client connection has a token bucket on the relay's *read* side (`Limits::client_rx`: `bytes_per_second`, `max_burst_bytes`). When the bucket is empty, the relay stops polling that socket until it refills. Nothing is dropped and the connection is not closed.
- **Notification** (`iroh-relay/src/protos/relay.rs`, `Status::RateLimited`, wire discriminant `2`): the relay sends it "once per connection when the relay first throttles reading from the client" (`server/client.rs`). This was added in 1.1.0 (#4455). The docs say only clients at 1.0.4+ see it, and older clients are throttled identically.
- **Client side** (`iroh/src/socket/transports/relay/actor.rs`, ~line 762): on `Status::RateLimited` iroh does exactly two things:
  1. `warn!` with the message *"The relay is rate-limiting this endpoint; outbound relay traffic is being throttled. Send less data over the relay, or, if you operate this relay, raise Limits::client_rx. Read more about rate limiting at https://docs.iroh.computer/relays/rate-limiting"*.
  2. It increments `SocketMetrics::relay_conns_ratelimited` once per relay connection.
- **How the app can observe it:** `endpoint.metrics().socket.relay_conns_ratelimited` (a `Counter`; the `metrics` feature is on by default), or a tracing subscriber filtered on that warn line. `Endpoint::home_relay_status()` → `RelayStatus` exposes only `url()`, `is_connected()`, `last_error()`, `auth_denied_reason()` (1.2.0). **Rate limiting is not exposed there** and there is no event or callback.
- **Backpressure path:** `RelaySender::poll_send` uses `PollSender::poll_reserve` (`transports/relay.rs`), so a stalled relay write turns into QUIC send backpressure. Congestion control then slows the Transfer. It does not error.
- **Docs** (docs.iroh.computer/relays/rate-limiting): "This is not an error… the connection stays open and traffic keeps moving, just slower than usual." "Relay rate limits only apply while traffic is relayed… A successful direct P2P path bypasses the relay entirely."
- **Public relay limits:** "The exact rate limits aren't published today, as they get tuned as needed." The rate-limiting page says the mechanism is per-connection. The Shared Relays page instead says the public relays "are rate-limited based on total traffic across every iroh user on them". The two n0 pages disagree.
- For comparison, paid tiers (iroh.computer/pricing): the $19/mo Shared/Pro tier has a 5 MB/s rate limit, 100 GB egress, and auth. Dedicated (+$199/mo/region) has no rate limits. Public is listed as "Variable".

### Inferred consequences [I]

- Only the **Sender's upload into the relay** is limited (rx from the client's point of view). A Receiver downloading via relay is slowed only because the Sender is.
- The relay client sends pings with a 5 s `PING_TIMEOUT` (`iroh-relay/src/ping_tracker.rs`) and treats any frame write stuck for longer than 15 s as `SendTimeout` (`run_sending`). Under very heavy throttling, a ping can queue behind throttled data, which would make the client drop and reconnect the relay connection. The QUIC connection itself should survive a short relay reconnect, because path idle timeout is 15 s and heartbeats run every 5 s (`socket.rs`), but a long outage on a relay-only path could close it. This is not documented. **Test it** with a self-hosted `iroh-relay` configured with a low `[limits.client.rx]`.
- For BhayanakShare, a relay-only Transfer (both sides behind symmetric NAT or blocked UDP) will be slow but should complete. The UI can only say "throttled by relay" by polling the metrics counter or hooking logs.

## 3. Is pkarr/DNS publishing required to reach a Contact by Device ID?

### Verified

- `Builder::address_lookup` / `clear_address_lookup` docs: "If no Address Lookup is set, connecting to an endpoint without providing its direct addresses or relay URLs will fail." The error is `ConnectWithOptsError::NoAddress` ("No addressing information available").
- With `presets::N0`, dialing a bare `EndpointId` works only because the *Receiver* publishes its home relay to `dns.iroh.link` and the *Sender* resolves it there (docs: connecting/dns-address-lookup). "Two endpoints must publish to and resolve from the same server to find each other."
- A **relay URL alone is enough to connect.** iroh exchanges direct addresses over the relay and hole-punches from there (concepts/relays, concepts/address-lookup: "As long as iroh has an EndpointID and its associated relay URL… we can dial that endpoint").

### Alternatives [V that they exist; I for fit]

| Option | What it is | Trade-off |
|---|---|---|
| Mainline DHT | `iroh-mainline-address-lookup` 0.6.0 (depends `iroh ^1.0.0`), `DhtAddressLookup::builder()`. Can be stacked with N0. Publishes the same signed record to BEP-44, republishing hourly. | No n0 server. Lookups are slower than DNS. The FAQ notes that it makes the app look like a BitTorrent DHT participant, which is why it is off by default. |
| mDNS | `iroh-mdns-address-lookup` 0.6.0 | LAN only. Already relevant to Nearby Devices. |
| Stored address | Persist each Contact's last `EndpointAddr` (relay URL, optionally IPs) after any successful connection. Pass it to `connect` or `MemoryLookup`. | Goes stale if the Contact's home relay changes. With only 4 n0 relays chosen by latency, it changes rarely for a given location [I]. |
| Ticket or link | `iroh-tickets` 1.0.0 `EndpointTicket` (id + relay URL + optionally IPs), or a custom link embedding the relay URL. | Longer than a bare Device ID. Can go stale. Embedding IPs leaks them to whoever holds the link. |
| Multiple lookups | `address_lookup()` can be called repeatedly. `AddressLookupServices` queries all of them concurrently. | Redundancy: N0 DNS + DHT + stored address. |

**Recommendation [I]:** keep N0 DNS as primary. Also store each Contact's last-seen relay URL locally and feed it through `MemoryLookup`. That way a `dns.iroh.link` outage or rate limit does not stop reconnection to known Contacts. Consider the DHT as an opt-in fallback.

Note on `dns.iroh.link` rate limits: the open-source `iroh-dns-server` throttles `PUT /pkarr` per client IP (governor: 1 token per 4 s, burst 2; `http/rate_limiting.rs`). `config.prod.toml` uses `"smart"` IP extraction. Whether n0's instance uses those values is not published [V for code, unknown for n0 prod]. A publisher republishes every 5 min, so many Devices behind one NAT are unlikely to hit it in normal use [I].

## 4. Published terms of use

- **No dedicated ToS or fair-use policy** exists for the public relays or `dns.iroh.link`. I checked `iroh.computer/{terms,tos,terms-of-service,acceptable-use}` and `n0.computer/terms` (all 404). `services.iroh.computer/terms` is a JS app with no readable content.
- **iroh.computer/legal** has a short "Terms of Service" for *Iroh Services*: lawful use only, no illegal content or DoS, availability not guaranteed, provided "as is" with no liability, and terms may change. Whether it covers the free public relays is not stated, although the docs place public relays under `/iroh-services/relays/public` [I].
- **Public Relays support policy** (docs.iroh.computer/iroh-services/relays/public):
  - "suitable for **development and hobby use only**. For production, use managed relays."
  - No SLA or uptime guarantee.
  - Only the latest stable iroh is officially supported, and "n0.computer reserves the right to remove support for older iroh versions at any time".
  - Traffic is rate-limited.
  - Relays see metadata (IPs, times, byte counts). n0 recommends against relaying sensitive data. n0 monitors for abuse and "reserve[s] the right to block offending IP addresses or users".
- **Release policy:** "Number 0 runs public relays for the **latest major version** of iroh." The 1.x wire protocol is compatible within 1.x and with 2.x.
- **Concepts/Relays:** "Public relays are suitable for development and testing."
- **DNS page wording differs:** "You're more than welcome to run production systems using the public relays if you find performance acceptable. The public servers do rate-limit traffic, there is no guaranteed uptime." This appears in the DNS/pkarr guide and conflicts with the relay pages [V for the text].
- **FAQ:** "What if number 0 stops running relay servers? You're not dependent on us… running your own is possible."

**Implications [I]:** shipping v1 on public infra is tolerated but explicitly unsupported. The concrete risks are:
- (a) Throttled relay-only Transfers.
- (b) Old app versions losing relay support when n0 upgrades. An auto-update path matters.
- (c) An abuse block applied by IP could hit unrelated users behind a shared NAT.
- (d) No notice period for changes.

## 5. Can relay and DNS URLs become configurable later without breaking compatibility?

### Relays: yes [V]

- `RelayActor::active_relay_handle_for_endpoint` / `start_active_relay` (`relay/actor.rs`) open a connection to **any** relay URL learned for a remote, whether or not it is in the local relay map. The relay map only decides the home relay, plus the auth token for listed relays.
- FAQ: "Running your own relay doesn't affect interoperability. Your endpoints can still connect to peers using other relay servers."
- Device A on a custom relay publishes that relay URL in its pkarr record. Device B on N0 defaults resolves the record and connects to A's relay.
- **Caveat:** if the custom relay requires auth (Iroh Services Shared/Dedicated relays authenticate by default with a project API key), Devices without a token cannot reach Devices homed there. n0's FAQ advises against shipping API keys in consumer apps. So a future custom relay for a consumer app must be unauthenticated (with rate limits) or use its own access control that admits every BhayanakShare Device [V for auth facts; I for consequence].
- Wire compatibility: the relay protocol is versioned. Unknown `Status` values decode as `Unknown(n)` and are logged. Within 1.x, peers and relays must stay compatible (release policy).

### DNS/pkarr: only if overlap is maintained [V + I]

- "Two endpoints must publish to and resolve from the same server to find each other" (dns-address-lookup guide).
- The record format (`_iroh.<z32>` TXT with `relay=` / `addr=`) is a fixed spec, so any `iroh-dns-server` is interchangeable.
- Safe migration [I]: add a second `PkarrPublisher::builder(url)` and resolver alongside the N0 ones, so Devices publish to and resolve from both. Only drop `dns.iroh.link` once all Devices in the field publish to the new server. Devices that switch exclusively to a different server become unreachable by bare Device ID from Devices still on N0, and the reverse is also true. Stored relay URLs (section 3) soften this.
- Implementation note: to make URLs configurable, build from `presets::Minimal` and add `RelayMode::Custom(RelayMap)` + `PkarrPublisher` + `PkarrResolver` + `DnsAddressLookup` explicitly. `presets::N0` is a 20-line function that can be copied (presets.rs doc comment recommends copying it).

## Other n0-related gotchas worth knowing [V]

- `Status::SameEndpointIdConnected`: if two processes use the same secret key, the relay delivers to only one. A second app instance or a cloned config would silently lose relay traffic.
- Relays see EndpointIds, IPs and byte counts for relayed connections. n0 says it does not record them (FAQ). Content is end-to-end encrypted.

## Sources

- iroh source, tag v1.3.0: https://github.com/n0-computer/iroh/tree/v1.3.0
  - `iroh/src/endpoint/presets.rs`, `iroh/src/defaults.rs`, `iroh/src/endpoint.rs` (RelayStatus, home_relay_status, metrics, address_lookup docs, NoAddress, default_relay_mode)
  - `iroh/src/socket/transports/relay/actor.rs`, `iroh/src/socket/transports/relay.rs`, `iroh/src/socket/metrics.rs`, `iroh/src/socket.rs`
  - `iroh/src/address_lookup.rs`, `iroh/src/address_lookup/{pkarr,dns}.rs`
  - `iroh-relay/src/protos/relay.rs`, `iroh-relay/src/server.rs`, `iroh-relay/src/server/{streams,client}.rs`, `iroh-relay/src/ping_tracker.rs`
  - `iroh-dns-server/src/http/rate_limiting.rs`, `iroh-dns-server/config.prod.toml`, `iroh-dns-server/src/config.rs`
  - `CHANGELOG.md` (1.1.0 #4455 rate-limit notice; 1.2.0 #4501 `auth_denied_reason`; 1.0.3 #4412 pkarr resolver in N0)
- iroh-mainline-address-lookup 0.6.0 source (crates.io download), `src/lib.rs`
- docs.iroh.computer: `/relays/rate-limiting`, `/iroh-services/relays/public`, `/iroh-services/relays/shared`, `/concepts/relays`, `/concepts/address-lookup`, `/concepts/tickets`, `/connecting/dns-address-lookup`, `/connecting/dht-address-lookup`, `/add-a-relay`, `/configuring-networks`, `/about/release-policy`, `/about/faq`
- https://www.iroh.computer/legal (Iroh Services ToS), https://www.iroh.computer/pricing
- crates.io API: iroh 1.3.0, iroh-tickets 1.0.0, iroh-mainline-address-lookup 0.6.0, iroh-mdns-address-lookup 0.6.0
