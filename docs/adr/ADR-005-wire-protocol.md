# ADR-005: Extend the wire protocol rather than adopt Scream's

- Status: accepted
- Date: 2026-08-27

## Context

Scream's format is documented byte for byte in `protocol.md`, derived from the
MS-PL driver source. The question was whether to speak it unchanged.

## The problem

Scream's header is **five bytes with no spare bits**:

| Byte | Content | Spare bits |
| --- | --- | --- |
| 0 | rate marker: bit 7 base, bits 0-6 multiplier | none |
| 1 | bits per sample, literal value | none usable |
| 2 | channel count, literal value | none usable |
| 3-4 | 16-bit channel mask, every bit meaningful | none |

**There is no compatible way to extend it in place.**

What it does not carry matters more:

| Missing | Consequence |
| --- | --- |
| **Sequence number** | Loss, reordering and duplication are all undetectable. The packet-loss figure the product promises is unimplementable. |
| **Timestamp** | RFC 3550 jitter needs the sender's timestamp for `D(i,j)`. Without it there is nothing for an adaptive buffer to adapt on. |
| **Clock reference** | Drift can only be inferred indirectly from buffer level. |
| **Magic value** | A receiver bound to the port decodes any 1157-byte datagram as audio. |
| **Version field** | The format can never change compatibly. |
| **Keepalive** | Silence suppression and a crashed sender are indistinguishable. |

Every headline feature of this product - a latency figure, a packet-loss
figure, an adaptive buffer, drift correction - is impossible on the format as
it stands.

## Decision

**Speak both. Send Sonduit's own format; receive either.**

### `SonduitPacket`, 20-byte header

```text
 0..4   magic "SDT1"
 4      version
 5      flags
 6..8   sequence number, wrapping
 8..12  timestamp: frames elapsed on the sender's sample clock
12      sample rate marker, encoded exactly as Scream does
13      bits per sample
14      channel count
15      reserved, must be zero
16..18  channel mask
18..20  payload length in bytes
20..    PCM payload
```

Design notes:

- **The timestamp is a frame count, not wall-clock time.** That is precisely
  what a drift estimator needs: comparing sender frames against frames the
  receiver has actually consumed measures the difference between the two sample
  clocks directly, with no clock synchronisation required.
- **The format fields keep Scream's encoding.** One decoder handles both, and
  `protocol.md` stays the single description of how a rate is encoded.
- **A length field** means a short final packet is expressible, which Scream's
  fixed size cannot do.
- **Version plus flags plus a reserved byte** gives room to extend, which is
  the whole point.
- Total for a full payload is 1172 bytes, still far below any MTU. Datagrams
  must never fragment, because a fragmented datagram is lost entirely if any
  fragment is.

### `ScreamPacket` stays, for compatibility

An unmodified Scream driver works as a sender out of the box. This matters more
than it looks given ADR-002: while Sonduit has no signed driver of its own, a
user who already runs Scream can use Sonduit as a receiver today.

Classification checks **magic before length**, because a Sonduit packet can
legitimately be 1157 bytes and must not be decoded as Scream. There is a test
for exactly that case.

## Consequences

- Two encoders and two decoders, both in `sonduit-core::packet`, both tested
  including truncated and malformed input.
- Receiving Scream means accepting degraded service: no loss detection, no
  jitter estimate, a fixed buffer depth. The UI must show that honestly rather
  than displaying a zero loss rate that means "unmeasurable".
- 20 bytes of header on 1152 bytes of payload is 1.7% overhead. Irrelevant.
- Silence suppression, if a Scream sender has it enabled, looks identical to
  total loss. Any timeout-based "connection lost" logic must account for it.

## Alternatives rejected

- **Adopt Scream unchanged.** Rejected: it makes the product's own feature list
  impossible.
- **Replace it entirely and drop compatibility.** Rejected: interoperating with
  an existing signed driver is genuinely valuable while we have none.
- **Use RTP.** Tempting, since RFC 3550 is already the jitter model. Rejected
  for now: RTP brings SSRC handling, payload type negotiation and RTCP, none of
  which a single-sender point-to-point link needs. Worth revisiting if Sonduit
  ever needs multiple senders or standard tooling; the header above deliberately
  carries the same two fields RTP would.
