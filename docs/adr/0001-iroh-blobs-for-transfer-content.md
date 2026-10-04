# Transfer content moves via iroh-blobs; negotiation uses our own protocol

A Transfer is negotiated over our own small protocol on a dedicated ALPN (Offer, accept, decline), after which the Receiver pulls the content with iroh-blobs. We chose this over a custom content-streaming protocol on raw iroh QUIC streams because iroh-blobs already gives BLAKE3-verified streaming, range requests and a persistent fs-store, which together make resume-across-restarts nearly free.

## Consequences

- iroh-blobs (0.103 at time of writing) is pre-1.0 and its README warns it is not yet production quality; its collection format is marked "subject to change". We accept upgrade churn and pin an exact version. The iroh-blobs ALPN has stayed `/iroh-bytes/4` across incompatible request changes and has no version negotiation, so a protocol version belongs in the Offer.
- A Collection is only a flat list of names and hashes. Folder metadata and checks on received names are ours to build.
- Content is pulled by the Receiver, not pushed by the Sender, even though the user-facing flow is push-shaped.
