# ADR-006: UDP broadcast discovery, not mDNS

- Status: accepted; the "deliberately excluded for now" consequence was
  **answered on 2026-08-28** and the protocol was **amended** with it
- Date: 2026-08-27
- Amended: 2026-08-28, see [Consequences](#consequences)
- Superseded in part by [ADR-009](ADR-009-audio-encryption.md): discovery is
  version 3, carries two further message kinds, and now agrees a session key

## Context

The desktop app must find the phone. The two obvious options are mDNS/DNS-SD,
which is the standard answer, and a small purpose-built UDP exchange.

## Decision

**A minimal UDP probe and announce, scoped to one interface.**

- Magic `SDDS`, version byte, kind byte.
- Probe: 6 bytes. Announce: adds the audio port and a length-prefixed UTF-8
  name, capped at 63 bytes.
- Port 4011, next to the audio port.
- The address to stream to is **the address the announcement came from**,
  combined with the port it advertised.

## Why not mDNS

1. **Interface scoping is the whole problem, and mDNS makes it harder.**
   USB tethering hands out a random `/24` (ADR-004), and with Wi-Fi and USB
   both up the PC has two routes to the same phone. Discovery must go out of
   one specific adapter and the answer must be interpreted relative to it. A
   raw socket bound to a chosen local address does that in one line. mDNS
   libraries generally want to manage interfaces themselves.

2. **Dependency weight against a tiny protocol.** The whole exchange is about
   130 lines of implementation plus its tests. An mDNS crate is a significant dependency for a
   two-message protocol, and every dependency has to be justified.

3. **Reliability on the platforms that matter.** Windows has its own mDNS
   responder that can conflict with a library-based one, and Android requires
   a multicast lock for reliable mDNS reception. Both are avoidable problems.

4. **There is nothing to browse.** DNS-SD earns its complexity when a user
   picks among many services of many types. Sonduit has one service and
   usually one device.

## Consequences

- **Sonduit devices are not discoverable by generic tools.** Accepted; the
  Android app is the only client.
- Broadcast can be blocked on some networks, particularly on guest or isolated
  Wi-Fi. A manual address entry is required as a fallback and is in the
  roadmap.
- The protocol is versioned from the first commit, so it can be replaced.
- ~~Deliberately excluded for now: authentication and pairing.~~ **Answered,
  in two steps.** Version 2 added a per-probe nonce and an HMAC-SHA256 keyed by
  a six-digit pairing code, so nothing on the LAN can answer a probe and be
  selected. Version 3 ([ADR-009](ADR-009-audio-encryption.md)) added the two
  further message kinds that agree a session key, so nothing on the LAN can
  read the audio or send audio to a listening receiver either.

  Being versioned from the first commit is what made both steps cheap: a peer
  that speaks an older version is not answered, which is a loud refusal rather
  than a silent downgrade. The three versions are why that byte is there.

## Revisit if

- Sonduit gains multiple concurrent receivers, or an interoperability
  requirement with third-party software.
- Broadcast turns out to be blocked commonly enough in practice that manual
  entry becomes the normal path rather than the fallback.
