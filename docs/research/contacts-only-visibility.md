# Can a Device be visible to Contacts only on the LAN?

Research for [issue #3](https://github.com/theHimanshuShekhar/bhayanakshare/issues/3). Researched 2026-10-03 against iroh 1.3.0, iroh-mdns-address-lookup 0.6.0, swarm-discovery 0.6.3, iroh-mdns-peer-lookup 0.1.2, hick 0.2.0 / hick-reactor 0.2.0 / hick-udp 0.1.0 / mdns-proto 0.3.0, and iroh-dns 1.3.0. Crate sources were downloaded from static.crates.io and read directly.

Each claim is tagged **[V]** (verified in the cited primary source) or **[I]** (inference or engineering judgement, not tested).

## Answer

**Yes, with one limit.** A Device can be passively invisible to LAN observers who do not already know its Device ID, while Devices that know the ID still find it nearby. No design can hide a Device's presence from someone who already holds its Device ID. The glossary requires that "a Device can still receive a Transfer from someone who has its Device ID", so any ID holder can always dial it and see whether it answers over a direct LAN path. The privacy boundary that can actually be enforced is therefore **people who know my Device ID** versus **people who don't**.

Recommendation:

1. **Discovery engine: `swarm-discovery` (the engine under `iroh-mdns-address-lookup`), driven directly by the app.** Use an app-specific service name and per-interface sockets, and feed the results into iroh through `MemoryLookup`. Do not use `iroh-mdns-peer-lookup`: it has no licence, cannot browse, uses a single interface chosen by enumeration order, and has 378 downloads.
2. **Everyone**: announce in plaintext (Device ID as the instance label). The Device Name is shown to everyone on the LAN.
3. **Contacts only**: send a *blinded beacon*. The instance label is `H(Device ID, time epoch)`, a rotating, random-looking value. The TXT payload holds the iroh port(s), sealed under a key derived from the Device ID. Listeners recompute the expected labels for the Device IDs they hold. Cost is one beacon per Device whatever the number of Contacts, with no custom asymmetric crypto. In practice the Device is visible to **Devices that know its Device ID**. That is mostly the Devices that saved it as a Contact, which is the *reverse direction* from "Devices it saved". See the decision questions.
4. **Hidden**: send no beacon. The Device stays reachable by ID over iroh's internet path (n0 DNS + relay, which then upgrades to a direct LAN path). It is **not** reachable on an offline LAN.

If the product requires "visible only to the Devices *I* saved" (one-sided, from the announcer's side), the only passive design is a **sealed beacon**: one HPKE-style key wrap per Contact. It works, but adds custom crypto, a capacity limit of about 15–20 Contacts per mDNS packet, and leaks the size of the Contact list. See Design D3.

## 1. mDNS crate comparison

### iroh-mdns-address-lookup 0.6.0 (n0) → swarm-discovery 0.6.3

- **[V]** It wraps `swarm_discovery::Discoverer::new_interactive` (τ = 0.7 s cadence, φ = 2.5 Hz response rate). The default service name is `irohv1`. The instance label is the lowercase base32 Device ID: `<id>._irohv1._udp.local`. ([iroh-mdns-address-lookup src/lib.rs](https://docs.rs/crate/iroh-mdns-address-lookup/0.6.0/source/src/lib.rs); [swarm-discovery src/lib.rs](https://docs.rs/crate/swarm-discovery/0.6.3/source/src/lib.rs))
- **[V]** It publishes all direct IPs and ports, the first relay URL (TXT `relay=`), and the iroh `UserData` (TXT `user-data=`, at most 245 bytes per [iroh-dns 1.3.0 `UserData::MAX_LENGTH`](https://docs.rs/crate/iroh-dns/1.3.0/source/src/endpoint_info.rs)). An `AddrFilter` can restrict which addresses are published.
- **[V]** `advertise(false)` stops publishing, but the Discoverer still sends PTR queries for `_irohv1._udp.local.` every cycle. The queries carry no identity, but the source IP and MAC are visible. ([swarm-discovery src/sender.rs](https://docs.rs/crate/swarm-discovery/0.6.3/source/src/sender.rs))
- **[V]** `resolve(id)` sends no targeted query. It only waits up to 10 s for that ID to appear in the passive swarm traffic.
- **[V]** It browses: `subscribe()` streams `Discovered` and `Expired` events for every endpoint on the service. This is what a Nearby list needs.
- **[V] Multi-interface is not wired up.** `MdnsAddressLookup` never calls `with_multicast_interfaces_v4`. As a result swarm-discovery's wildcard v4 socket joins 224.0.0.251 **only on the default-route interface**, and IPv6 always uses the default interface ([swarm-discovery src/socket.rs comments](https://docs.rs/crate/swarm-discovery/0.6.3/source/src/socket.rs)). The fix is the still-open [iroh-address-lookups PR #7](https://github.com/n0-computer/iroh-address-lookups/pull/7), which is IPv4 only, opened 2026-06-11 and stalled on CI. Its description says that when a second interface holds the default route ("a second uplink, a VPN, a tethered connection"), "A and B never discover each other. The failure is silent".
- **[V]** swarm-discovery itself does support multiple interfaces (`with_multicast_interfaces_v4`, `DropGuard::add_interface_v4` / `remove_interface_v4`). These arrived in [PR #17](https://github.com/rkuhn/swarm-discovery/pull/17), and per-interface egress pinning and group joins were fixed in [PR #25](https://github.com/rkuhn/swarm-discovery/pull/25) (0.6.2, 2026-06-12).
- **[V] Known defects:**
  - In 0.6.0, `Discovered` is re-emitted on every response, because `Peer` equality includes `last_seen`. The cached address can also go stale. Both are fixed by the open [PR #15](https://github.com/n0-computer/iroh-address-lookups/pull/15).
  - It is slow to "win the race" against relay lookup ([issue #13](https://github.com/n0-computer/iroh-address-lookups/issues/13), [#14](https://github.com/n0-computer/iroh-address-lookups/issues/14)).
  - An acto tracing panic was fixed in swarm-discovery 0.6.1 ([swarm-discovery #24](https://github.com/rkuhn/swarm-discovery/issues/24)). [iroh-address-lookups #4](https://github.com/n0-computer/iroh-address-lookups/issues/4) is still open, but 0.6.3 locks acto 0.8.2.
- **[V]** By its own README, swarm-discovery does not aim to interoperate with other mDNS stacks ([#11](https://github.com/rkuhn/swarm-discovery/issues/11)).
- **[V]** The receiver accepts any packet with no source-port or TTL check, so anyone on the LAN can spoof announcements ([src/receiver.rs](https://docs.rs/crate/swarm-discovery/0.6.3/source/src/receiver.rs)). The receive buffer is **1472 bytes**, so larger responses are truncated.
- **[V]** Downloads: about 213k (iroh-mdns-address-lookup) and about 480k (swarm-discovery). Licence: MIT/Apache-2.0.
- **[V]** Field reports:
  - Windows failed to discover anything while Linux and macOS worked ([iroh #3310](https://github.com/n0-computer/iroh/issues/3310), 2025, pre-0.6, still open).
  - Discovery failed across a Wi-Fi hotspot until multi-interface support was added ([iroh #3533](https://github.com/n0-computer/iroh/issues/3533)).
  - Wi-Fi client isolation or multicast filtering breaks discovery ([iroh #3084](https://github.com/n0-computer/iroh/issues/3084)).

### iroh-mdns-peer-lookup 0.1.2 (third party, codeberg nfnitloop/sned) → hick 0.2

- **[V] No licence:** the LICENSE file reads `TBD (open to suggestions)`. Without a licence the crate cannot legally be redistributed. ([crate source](https://docs.rs/crate/iroh-mdns-peer-lookup/0.1.2/source/LICENSE))
- **[V]** Three versions were published on one day (2026-08-13), with 378 total downloads and one author.
- **[V] Resolve only, no browse.** It implements only `AddressLookup::resolve(id)`, which sends a targeted DNS-SD query for `<z32 id>._irohv1._udp.local.`. There is no subscribe or browse API, so it **cannot populate a Nearby list** of unknown Devices.
- **[V]** The query name contains the target Device ID in plaintext, so every passive listener learns whom you are looking for.
- **[V]** With `announce(true)` it registers a full RFC 6763 service through mdns-proto. That service answers PTR browse queries for `_irohv1._udp.local.` and `_services._dns-sd._udp.local.` enumeration from anyone ([mdns-proto 0.3.0 src/records.rs, src/service/mod.rs](https://docs.rs/crate/mdns-proto/0.3.0)), so any standard mDNS browser lists the Device.
- **[V]** `announce(false)` disables answering entirely, which makes it also unresolvable.
- **[V] One interface, picked by enumeration order rather than the default route.** hick-reactor binds "the first up + multicast-capable, non-loopback interface reported by `getifs::interfaces`", and "Multi-interface binding … is not yet supported" ([hick-reactor 0.2.0 src/options.rs, src/endpoint.rs](https://docs.rs/crate/hick-reactor/0.2.0)). `iroh-mdns-peer-lookup` does not expose `with_interface_index`.
- **[V]** It defaults to IPv4 only because of an upstream issue.
- **[I]** On Windows hosts with Hyper-V, WSL, VirtualBox or VPN adapters, and on Linux hosts with docker0, the "first" interface may be a virtual one, so discovery silently fails.
- **[V]** hick does have better hygiene: it drops responses whose source port is not 5353, enforces RFC 6762 §11 interface scoping, and uses `SO_REUSEADDR` (plus `SO_REUSEPORT` on Unix) to share 5353 ([hick-udp 0.1.0 src/platform/*.rs](https://docs.rs/crate/hick-udp/0.1.0)).

**Verdict [I]:** `iroh-mdns-peer-lookup` is disqualified (no licence, no browse, plaintext ID queries, single arbitrary interface). Use swarm-discovery, but handle multi-interface yourself, either by using swarm-discovery directly or by waiting for PR #7. Plan a cross-OS prototype: Windows has an open, unexplained failure report, and nobody has published a Windows success report for 0.6.x.

### Platform behaviour (applies to both crates)

- **Windows [V]:** "If there's no active application or administrator-defined allow rule(s), a dialog box prompts the user … the first time the app … tries to communicate in the network". A non-admin user gets block rules whatever they click ([MS Learn: Windows Firewall rules](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/rules)). **[I]** iroh's own UDP socket already binds `0.0.0.0` ([iroh 1.3.0 src/endpoint.rs](https://docs.rs/crate/iroh/1.3.0/source/src/endpoint.rs)), so the prompt appears whatever mDNS choice is made. The installer should add inbound rules (MS recommends staging them before first launch). **[I]** Networks classified as *Public* block inbound traffic unless the rule covers that profile, which is a common cause of "works at home, not at the café".
- **macOS [V]:** Local Network privacy (macOS 15+). "Sending or receiving multicast or broadcast traffic is a local network operation". The multicast entitlement "isn't required on macOS". Add `NSLocalNetworkUsageDescription` and `NSBonjourServices`. The system "may deny the operation immediately, before the user has responded", so retries are needed. Command-line tools run from Terminal are automatically allowed, so `tauri dev` from a terminal **will not show the prompt** and testing must use a signed `.app`. Identity is tracked by code signature, so the app must be signed with an Apple-issued identity. VPN interfaces are not "local networks". ([Apple TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy))
- **Linux [V]:** firewalld's upstream `public` zone allows only `ssh` and `dhcpv6-client`, while the `home` zone also allows `mdns` (UDP 5353 to 224.0.0.251 / ff02::fb) ([firewalld config/zones](https://github.com/firewalld/firewalld/tree/main/config/zones)). **[I]** On firewalld `public` or ufw `deny incoming`, inbound mDNS is dropped. Users must allow the `mdns` service, or the app must document it. Both crates share 5353 with Avahi through `SO_REUSEADDR`/`SO_REUSEPORT`.
- **VPNs [V/I]:** **[V]** swarm-discovery joins the group on the default-route interface only, unless interfaces are listed explicitly. **[I]** A full-tunnel VPN (or a Tailscale exit node) moves the default route, so LAN discovery stops until per-interface sockets are used. Many corporate VPNs also block LAN traffic outright, and nothing can fix that.
- **Wi-Fi [V]:** client isolation and multicast filtering on some access points break all mDNS ([iroh #3084](https://github.com/n0-computer/iroh/issues/3084), [swarm-discovery #11](https://github.com/rkuhn/swarm-discovery/issues/11)). Fallbacks are pasting the Device ID, or the internet path.

## 2. What leaks regardless of design

- **[V] The Device ID leaks to anyone who can reach iroh's UDP port.** In iroh releases after 1.0.3 the client no longer sends SNI ("Iroh servers do not use SNI, so do not disclose the endpoint ID in the ClientHello"), and the server certificate is the raw Ed25519 key ([iroh 1.3.0 src/tls.rs, src/tls/name.rs, src/tls/verifier.rs](https://docs.rs/crate/iroh/1.3.0/source/src/tls.rs)).
  - **[I]** A TLS 1.3 server sends its certificate before it sees the client's, so any LAN host that finds the port can start a QUIC handshake and read the Device ID.
  - **[I]** This holds in *every* Visibility mode, including Hidden. Announcing the port makes it trivial; otherwise an attacker needs a UDP port scan. The privacy boundary is therefore **passive** observers. Active attackers on the LAN can identify any Device.
- **[I] Presence leaks to Device ID holders.** Anyone with the ID can dial the Device (that is the product rule) and see whether a direct LAN path forms, so no mode hides presence from them.
- **[I] Network identity leaks.** Any mDNS packet a Device sends, including the anonymous queries of a listening-only Device, exposes its IP and MAC and the fact that it runs this app (through the service name). Per-network MAC randomisation does not help within one network.
- **[V] Device Name in `UserData` reaches every lookup service.** `UserData` is published "together with the endpoint's addresses" to *every* configured lookup service, including n0 DNS/pkarr (TXT `user-data`) ([iroh src/endpoint.rs `user_data_for_address_lookup`; iroh-dns src/endpoint_info.rs](https://docs.rs/crate/iroh-dns/1.3.0/source/src/endpoint_info.rs)). **[I]** Putting the Device Name there publishes it globally to anyone who knows the ID. Keep the Name out of `UserData` and carry it in the app's own beacon or control protocol.
- **[V/I] The default `irohv1` service name mixes in other iroh apps.** **[V]** iroh-mdns-address-lookup defaults to `irohv1`. **[I]** With that name the Nearby list would include any iroh app on the LAN (sendme, dumbpipe, …) and announce us to them. Use an app-specific service name of at most 15 characters (RFC 6335 limit).
- **[I] Self-asserted names enable spoofing.** Device Names from non-Contacts are self-asserted (announcements can be spoofed and are unauthenticated). A malicious Device can call itself "Alice's MacBook". Contacts are safe because the UI shows the saved Contact keyed by Device ID.

## 3. Designs for Contacts-only Visibility

Notation: B is the Device being hidden. A is an observer on the same LAN.

| Design | Who sees B as Nearby | Passive leak to non-ID-holders | Cost / limits |
|---|---|---|---|
| **D0** Plain announce (Everyone) | Everyone | Device ID, Name (if in TXT), IPs+ports, relay URL | none |
| **D1** No announce; Contacts dial B by ID (iroh global lookup + relay, then direct path; B answers a control-ALPN "hello" only for authenticated Contacts) | Mutual Contacts, **only with internet** | Nothing new | O(contacts) connections per poll; fails on offline LAN; strongest auth (QUIC mutual EndpointId) with no custom crypto |
| **D1-plain** Targeted mDNS lookup by ID (`iroh-mdns-peer-lookup` style) | Anyone with B's ID | **Queried IDs in plaintext**, and B's reply reveals ID + port | O(contacts) query names per poll; leaks the observer's Contact list |
| **D2 Blinded beacon** (recommended). B announces label `L = trunc(H("bhayanak-v1" ‖ ID ‖ epoch))`; TXT = AEAD under `K = KDF(ID)` of {ports, flags}; A recomputes L for every ID it holds | Anyone with B's ID (in practice, Devices that saved B) | App use + IP/MAC; rotating opaque label (no cross-network tracking); port hidden inside the ciphertext | 1 beacon per Device whatever the Contact count; A does O(\|ids held\|) hashes per epoch; anyone holding a candidate ID can confirm it (unavoidable) |
| **D3 Sealed beacon**. B announces a random label; TXT = ephemeral X25519 key + per-Contact wrap of a session key (HPKE-like, about 48 B per Contact before base64) + one ciphertext {ID, Name, ports} | Exactly **the Devices B saved**, even if they never saved B | App use + IP/MAC; **number of Contacts** (pad to buckets) | about 15–20 Contacts per packet (1472-byte receive buffer, TXT values must be UTF-8 so base64, at most 254 bytes per attribute); rotate subsets for more; custom asymmetric crypto (Ed25519→X25519 conversion) |
| **D4 Respond-only-to-Contacts**. A queries with tag `HMAC(DH(sk_A, pk_B), epoch)` per Contact; B answers (encrypted) only when the tag matches one of its Contacts | **Mutual** Contacts | App use + IP/MAC; querier's Contact-list size | O(\|C_A\|·\|C_B\|) DH/HMAC per round; swarm-discovery cannot do targeted queries, so this needs a custom mDNS layer (hick/mdns-proto) or plain UDP multicast |

Notes:

- **[V]** swarm-discovery only supports "browse everything" (one PTR query per service, and everyone answers with their full record). Targeted designs (D1-plain, D4) therefore need a different mDNS layer. Beacon designs (D0, D2, D3) fit swarm-discovery as-is, given a custom service name and a non-ID instance label. iroh-mdns-address-lookup cannot carry D2 or D3: it parses the instance label as a `PublicKey` and drops anything else.
- **[I] Why D2 over D3/D4 for v1:** the ID-holder leak already exists (section 2), so D3 and D4's extra exclusion of ID-holders who are not Contacts buys little. That is especially true online, where any ID holder can dial B through the relay and observe the path. D2 has constant size, no asymmetric crypto, and matches swarm-discovery's model.
- **[I] Epochs:** an epoch of 10–15 minutes, with listeners checking the current and adjacent epochs, tolerates clock skew. Each rotation looks like a new swarm member, and the old label expires after swarm-discovery's grace period.
- **[I] Wiring into iroh:** decode the beacon, then call `MemoryLookup::add_endpoint_info(id, addrs)` so that `endpoint.connect(id)` uses the LAN addresses ([iroh src/address_lookup/memory.rs](https://docs.rs/crate/iroh/1.3.0/source/src/address_lookup/memory.rs)). Changing Visibility means dropping and respawning the app's swarm-discovery `DropGuard`. Inside iroh, `AddressLookupServices` has only `add` and `clear` (no remove), and swarm-discovery configuration is fixed at spawn ([iroh #3945](https://github.com/n0-computer/iroh/issues/3945)). That is a further reason to own the Discoverer rather than register `MdnsAddressLookup`.

## 4. What each Visibility mode leaks to non-Contacts on the LAN (recommended design)

| Mode | Passive observer without B's ID | Holder of B's ID (not B's Contact) | Active attacker on LAN |
|---|---|---|---|
| Everyone (D0) | Device ID, Device Name, IPs, ports, relay URL, app use | same | same |
| Contacts only (D2) | App use, IP/MAC, beacon timing. **No ID, no Name, no port** | Presence, addresses (can show B as Nearby). Name only if B shares it over the control protocol | Device ID through a QUIC handshake after a port scan |
| Hidden | App use and IP/MAC only if B still sends listening queries (it could stop listening entirely). Nothing identifying | Presence only by dialing over the internet path; not on an offline LAN | Device ID through a QUIC handshake after a port scan |

## Sources

- iroh 1.3.0 source: `src/address_lookup.rs`, `src/address_lookup/memory.rs`, `src/endpoint.rs`, `src/tls.rs`, `src/tls/name.rs`, `src/tls/verifier.rs` (https://docs.rs/crate/iroh/1.3.0/source/)
- iroh-dns 1.3.0 `src/endpoint_info.rs` (https://docs.rs/crate/iroh-dns/1.3.0/source/)
- iroh-mdns-address-lookup 0.6.0 `src/lib.rs` (https://docs.rs/crate/iroh-mdns-address-lookup/0.6.0/source/)
- swarm-discovery 0.6.3 `README.md`, `src/{lib,sender,receiver,socket,updater,guardian}.rs` (https://docs.rs/crate/swarm-discovery/0.6.3/source/)
- iroh-mdns-peer-lookup 0.1.2 `README.md`, `LICENSE`, `src/lib.rs` (https://docs.rs/crate/iroh-mdns-peer-lookup/0.1.2/source/)
- hick-reactor 0.2.0 `src/options.rs`, `src/endpoint.rs`; hick-udp 0.1.0 `src/platform/{unix,windows}.rs`; mdns-proto 0.3.0 `src/config.rs`, `src/records.rs`, `src/service/mod.rs`
- GitHub: n0-computer/iroh-address-lookups PR #7, PR #15, issues #4, #13, #14; n0-computer/iroh issues #3084, #3310, #3401, #3533, #3945; rkuhn/swarm-discovery PRs #17, #25, issues #11, #24
- crates.io API (versions, downloads, publish dates), retrieved 2026-10-03
- Apple TN3179 "Understanding local network privacy" (rev. 2026-02-17)
- Microsoft Learn "Windows Firewall rules" (2025-06-06)
- firewalld `config/zones/public.xml`, `home.xml`, `config/services/mdns.xml` (main branch)

## Not verified

- No crate was run. Cross-OS behaviour (Windows especially), the VPN impact and the behaviour of the Windows firewall Public profile are inferred from source code and issue reports. They need a two-Device prototype on Windows/macOS/Linux.
- The claim that an active attacker can read the Device ID through a QUIC handshake is inferred from TLS 1.3 message order plus iroh's verified no-SNI design. It was not exercised.
