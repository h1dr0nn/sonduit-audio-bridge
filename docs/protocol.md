# Scream wire protocol

This document is the single source of truth for the wire format Sonduit speaks
on the network. Everything here was derived by reading the **Scream driver
source**, which is MS-PL licensed, and never from a GPL receiver
implementation. See [licensing.md](./licensing.md) for why that distinction
matters.

Source of every claim below, unless stated otherwise:
`third_party/reference/scream/Scream/` at commit
`d789743c248b11d1df7e5ecc546b1bc60b90cd91` (2026-08-11), specifically
`savedata.cpp`, `savedata.h`, `adapter.cpp`, `minstream.cpp` and `scream.h`.

The commit is named rather than left as "whatever is checked out". These are
claims about someone else's wire format, and if upstream changes it, an
unpinned reference would make this document look like a description of the new
one when it is a description of the old.

A wire format is a fact about bytes on a network, not a creative work. Reading
the sender to learn what bytes it emits is the same activity as capturing them
with a packet sniffer.

---

## 1. Transport

| Property | Value | Source |
| --- | --- | --- |
| Protocol | UDP, IPv4 | `savedata.cpp` `WskSocket(..., SOCK_DGRAM, IPPROTO_UDP, ...)` |
| Default destination | `239.255.77.77:4010` | `#define MULTICAST_TARGET` / `MULTICAST_PORT`, `savedata.cpp:9-10` |
| Datagram size | exactly **1157 bytes** | `CHUNK_SIZE = PCM_PAYLOAD_SIZE + HEADER_SIZE`, `savedata.cpp:13` |
| Source port | ephemeral by default | `g_UnicastSrcPort` defaults to `0`, `adapter.cpp` |
| Multicast TTL | not set unless configured | `g_TTL` defaults to `0`; `IP_MULTICAST_TTL` is only applied when non-zero, `savedata.cpp:365-368` |

`239.255.77.77` is in the IPv4 Administratively Scoped Block
(`239.0.0.0/8`, RFC 2365). With the default TTL of 1 the traffic does not
cross a router.

### 1.1 Datagrams are always the same size

`CSaveData::SendData` refuses to transmit a partially filled chunk:

```c
// savedata.cpp, SendData()
if ((storeOffset >= m_ulSendOffset) && ((storeOffset - m_ulSendOffset) < CHUNK_SIZE))
    break;
...
wskbuf.Length = CHUNK_SIZE;
```

So a receiver may treat any datagram whose length is not 1157 as malformed.

There is one edge case worth knowing: the guard only applies when
`storeOffset >= m_ulSendOffset`. On the ring-buffer wrap the driver sends a
chunk unconditionally, so a chunk that straddles the wrap can go out holding
stale bytes from the previous lap. This is rare and the driver's own comment
acknowledges the asymmetry. Sonduit's receiver must not assume payload bytes
are always fresh.

### 1.2 Unicast mode

Unicast is configured entirely through the registry, read once at driver load
(`adapter.cpp`, `GetRegistrySettings`). There is no runtime control channel.

Key: `HKLM\SYSTEM\CurrentControlSet\Services\Scream\Options`

| Value | Type | Default | Meaning |
| --- | --- | --- | --- |
| `UnicastIPv4` | `REG_SZ` | `239.255.77.77` | Destination address. Setting a normal host address turns the stream into unicast. |
| `UnicastPort` | `REG_DWORD` | `4010` | Destination port. |
| `UnicastSrcIPv4` | `REG_SZ` | unset | Local address to bind, i.e. which interface to send from. |
| `UnicastSrcPort` | `REG_DWORD` | `0` (ephemeral) | Local port to bind. |
| `TTL` | `REG_DWORD` | `0` (unset) | Applied as `IP_MULTICAST_TTL` when in range 1-255. |
| `DSCP` | `REG_DWORD` | `0` | **Read but not applied.** See below. |
| `SilenceThreshold` | `REG_DWORD` | `0` (off) | Consecutive silent samples before the driver stops sending. |
| `Version` | `REG_DWORD` | `0` | Read into `g_ScreamVersion`. |

Note that the driver names everything "Unicast" but the same values drive
multicast; the destination address alone decides which it is.

**DSCP does not work.** The value is parsed and masked to 6 bits, but the
call that would apply it is commented out:

```c
// savedata.cpp:370
// if (g_DSCP) status = SetSockOpt(m_socket, IPPROTO_IP, IP_TOS, (g_DSCP << 2) & 0xff);
//   no support in kernel - raw socket and IP_HDRINCL?
```

This matters for Sonduit: **we cannot get WMM/WMM-AC voice queueing on WiFi
by asking the stock Scream driver to mark packets.** DSCP marking would have
to come from somewhere else. Recorded in ADR-002.

### 1.3 `UnicastSrcIPv4` is the USB tethering lever

Because the driver binds its socket to `UnicastSrcIPv4`, pointing that at the
USB tethering interface's local address is how the stock driver would be made
to send over USB rather than WiFi. It requires a driver reload to change.

---

## 2. Packet layout

```
 byte   0        1        2        3        4        5                     1156
      +--------+--------+--------+--------+--------+----------------------------+
      |  rate  |  bits  |  chans |  mask  |  mask  |   1152 bytes of PCM        |
      | marker | /sample|        |   lo   |   hi   |                            |
      +--------+--------+--------+--------+--------+----------------------------+
       <------------- 5 byte header ------------->  <------ payload ----------->
```

Written by `CSaveData::WriteData`:

```c
// savedata.cpp, WriteData(), "Start a new chunk" branch
m_pBuffer[offset]     = m_bSamplingFreqMarker;
m_pBuffer[offset + 1] = m_bBitsPerSampleMarker;
m_pBuffer[offset + 2] = m_bChannels;
m_pBuffer[offset + 3] = (BYTE)(m_wChannelMask    & 0xFF);
m_pBuffer[offset + 4] = (BYTE)(m_wChannelMask>>8 & 0xFF);
```

### 2.1 Byte 0 - sample rate marker

The encoding packs a base flag and a multiplier into one byte:

```c
// savedata.cpp, Initialize()
m_bSamplingFreqMarker = (BYTE)((nSamplesPerSec % 44100)
    ? (0   + (nSamplesPerSec / 48000))
    : (128 + (nSamplesPerSec / 44100)));
```

- **Bit 7** (`0x80`): base rate. `0` means 48000 Hz, `1` means 44100 Hz.
- **Bits 0-6**: integer multiplier of that base.

Decoding is therefore:

```
base       = (marker & 0x80) ? 44100 : 48000
multiplier =  marker & 0x7F
rate       = base * multiplier
```

A multiplier of `0` is not a valid rate and should be rejected.

| Rate (Hz) | Marker (hex) | Marker (dec) |
| --- | --- | --- |
| 44100 | `0x81` | 129 |
| 48000 | `0x01` | 1 |
| 88200 | `0x82` | 130 |
| 96000 | `0x02` | 2 |
| 176400 | `0x84` | 132 |
| 192000 | `0x04` | 4 |

Note the asymmetry in the driver's own condition: the branch is taken on
`nSamplesPerSec % 44100`, so a rate divisible by 44100 takes the *false* arm
and gets the 44100 base. A rate such as 88200 is divisible by 44100 and is
encoded on the 44100 base, as expected.

The driver advertises support for 44100 Hz through 192000 Hz
(`MIN_SAMPLE_RATE` / `MAX_SAMPLE_RATE`, `scream.h:67-68`).

### 2.2 Byte 1 - bits per sample

The literal value, not an enum: `16`, `24` or `32`.
`MIN_BITS_PER_SAMPLE_PCM` is 16 and `MAX_BITS_PER_SAMPLE_PCM` is 32
(`scream.h:65-66`).

### 2.3 Byte 2 - channel count

The literal channel count, 1 through 8
(`MIN_CHANNELS` / `MAX_CHANNELS_PCM`, `scream.h:63-64`).

### 2.4 Bytes 3-4 - channel mask

The **low 16 bits** of `dwChannelMask` from Microsoft's
`WAVEFORMATEXTENSIBLE`, little-endian (low byte first).

The truncation to 16 bits is not an accident of the wire format; the driver
stores the mask as a `WORD`:

```c
// savedata.h
WORD  m_wChannelMask;
```

The 18 standard `SPEAKER_*` positions do not all fit in 16 bits, so the two
highest (`SPEAKER_TOP_BACK_LEFT`, `SPEAKER_TOP_BACK_CENTER`,
`SPEAKER_TOP_BACK_RIGHT` occupy bits 15-17) are partly lost. In practice
only the first 16 positions survive.

| Bit | Value | Speaker |
| --- | --- | --- |
| 0 | `0x0001` | Front left |
| 1 | `0x0002` | Front right |
| 2 | `0x0004` | Front centre |
| 3 | `0x0008` | Low frequency |
| 4 | `0x0010` | Back left |
| 5 | `0x0020` | Back right |
| 6 | `0x0040` | Front left of centre |
| 7 | `0x0080` | Front right of centre |
| 8 | `0x0100` | Back centre |
| 9 | `0x0200` | Side left |
| 10 | `0x0400` | Side right |
| 11 | `0x0800` | Top centre |
| 12 | `0x1000` | Top front left |
| 13 | `0x2000` | Top front centre |
| 14 | `0x4000` | Top front right |
| 15 | `0x8000` | Top back left |

Stereo is `0x0003`. Sonduit only needs stereo for the first milestone.

### 2.5 Bytes 5..1156 - payload

1152 bytes of raw interleaved PCM, **little-endian signed integers**, sample
size given by byte 1. No padding, no framing, no compression.

1152 was chosen because it is divisible by 4, 6 and 8, so a whole number of
frames always fits for the common combinations of channel count and sample
width (`savedata.cpp:11`).

Frames per packet is derived, never transmitted:

```
bytes_per_frame  = channels * (bits_per_sample / 8)
frames_per_packet = 1152 / bytes_per_frame
```

This must divide evenly. If it does not, the format is not representable and
the receiver should reject the packet.

| Format | Bytes/frame | Frames/packet | Packet duration | Packet rate |
| --- | --- | --- | --- | --- |
| 48 kHz, 16-bit, stereo | 4 | 288 | **6.000 ms** | 166.7 /s |
| 48 kHz, 24-bit, stereo | 6 | 192 | 4.000 ms | 250.0 /s |
| 48 kHz, 32-bit, stereo | 8 | 144 | 3.000 ms | 333.3 /s |
| 44.1 kHz, 16-bit, stereo | 4 | 288 | 6.531 ms | 153.1 /s |
| 48 kHz, 16-bit, 5.1 | 12 | 96 | 2.000 ms | 500.0 /s |

**The 6 ms figure for the baseline format is a hard floor on packetisation
delay** and is carried into [latency-budget.md](./latency-budget.md).

---

## 3. Silence suppression

When `SilenceThreshold` is non-zero, `minstream.cpp` counts consecutive
near-silent samples and stops calling `WriteData` once the count passes the
threshold. A sample counts as silent when its absolute value is below
`SILENCE_SAMPLE_LEVEL`, scaled by width; the driver notes that a pure-silence
WAV still produces PCM values between -2 and +2, so an exact-zero test would
not work.

24-bit is explicitly not handled by the silence check
(`minstream.cpp:663`, "24-bit is not yet supported").

**Consequence for Sonduit:** with suppression on, the packet stream simply
stops. A receiver cannot distinguish that from total packet loss or from the
sender disappearing, because there is no keepalive and no explicit
end-of-stream. Any timeout-based "connection lost" logic must account for
this. Recorded in ADR-005.

---

## 4. What the protocol does not carry

This is the important part for Sonduit, and the reason ADR-005 does not adopt
the format unchanged.

| Missing | Consequence |
| --- | --- |
| **Sequence number** | Packet loss cannot be detected. Reordering cannot be detected or repaired. Duplicates cannot be dropped. The receiver cannot report a loss rate, so the telemetry the product promises is unimplementable. |
| **Timestamp** | RFC 3550 inter-arrival jitter cannot be computed, because `D(i,j)` needs the sender's timestamp. An adaptive jitter buffer has nothing to adapt on beyond raw arrival spacing. |
| **Clock reference** | Sender and receiver sample clocks cannot be compared, so drift can only be inferred indirectly from buffer fill level. |
| **Stream identity / session id** | Two senders on the same multicast group interleave into one stream with no way to separate them. |
| **Protocol version** | The header cannot be extended compatibly. All five bytes are spoken for and there is no reserved bit. |
| **Keepalive or end-of-stream** | Silence suppression and a crashed sender look identical. |
| **Any integrity check** | Beyond the UDP checksum, a corrupt payload is played as noise. |

The header has no spare bits. Byte 0 uses bit 7 for the base and needs bits
0-6 for the multiplier; bytes 1 and 2 carry literal values whose valid ranges
already span most of a byte; bytes 3-4 are a bit field with all 16 bits
meaningful. **There is no compatible way to extend this header in place.**

---

## 5. Reference decoder

Pseudocode. The real one lands in `sonduit-core` with tests.

```
fn decode(datagram: &[u8]) -> Result<Frame> {
    if datagram.len() != 1157 { return Err(BadLength) }

    let marker  = datagram[0];
    let base    = if marker & 0x80 != 0 { 44100 } else { 48000 };
    let mult    = (marker & 0x7F) as u32;
    if mult == 0 { return Err(BadRate) }
    let rate    = base * mult;

    let bits    = datagram[1];
    if !matches!(bits, 16 | 24 | 32) { return Err(BadWidth) }

    let channels = datagram[2];
    if channels == 0 || channels > 8 { return Err(BadChannels) }

    let mask = u16::from_le_bytes([datagram[3], datagram[4]]);

    let bytes_per_frame = channels as usize * (bits as usize / 8);
    if 1152 % bytes_per_frame != 0 { return Err(UnrepresentableFormat) }

    Ok(Frame { rate, bits, channels, mask, pcm: &datagram[5..1157] })
}
```

Note there is nothing to validate against: every byte combination that passes
the range checks above is indistinguishable from a legitimate packet. A
receiver bound to `0.0.0.0:4010` will happily decode any 1157-byte datagram
that arrives. Sonduit's own header (ADR-005) fixes this with a magic value.

---

## 6. Open questions

Things this document does **not** establish, and which are not safe to assume:

- The real-world distribution of inter-packet arrival times on WiFi. The
  driver sends from a work item, not a timer, so packets are emitted in
  bursts whenever the audio engine hands over a buffer rather than paced at
  6 ms. **The burst shape has not been measured** and should be, on real
  hardware, before the jitter buffer is tuned.
- Whether the shipped signed binaries in `Install/driver/` correspond exactly
  to the source in `Scream/`. Not verified; the repository ships prebuilt
  binaries without a reproducible build.
- Behaviour when the audio format changes mid-stream. The driver rebuilds
  the header per chunk, so the change appears without warning on the next
  packet. Sonduit's receiver must handle a format change on any packet
  boundary.

---

## 7. Sonduit pairing invite (QR)

Not part of Scream. This is Sonduit's own format, defined once in
`crates/sonduit-transport/src/invite.rs` and unit tested there, so the desktop
that prints it and the phone that reads it cannot drift apart.

### 7.1 What problem it solves

Discovery (section 8 of ADR-006, and `discovery.rs`) is a UDP broadcast probe.
On a network where the phone and the desktop are in different subnets the probe
never arrives: measured on the network this was built for, the phone sits on
`10.10.22.160/22` and the desktop on `10.10.0.61`, and neither can reach the
other by broadcast or by ping. The scan finds nothing at all.

The invite reverses the direction. The **desktop** displays a QR code; the
**phone** reads it with the camera and sends its announcement by **unicast**
straight to an address the code gave it. Unicast crosses a router. The desktop
takes the phone's address from the source address of that datagram, never from
anything inside it.

### 7.2 Grammar

```text
invite   = "SDQ1" ":" code ":" port ":" nonce ":" addresses
code     = 6DIGIT
port     = 1*5DIGIT              ; 1-65535, the discovery port
nonce    = 26BASE32              ; 16 bytes, RFC 4648 alphabet, unpadded
addresses = address *( "-" address )
address  = dotted-quad IPv4
```

Example, wrapped here only for the page width:

```text
SDQ1:482913:4011:AEJEMZ4JVPHP73W3XKMHMVBSCA:10.10.0.61-192.168.42.100
```

| Field | Meaning |
| --- | --- |
| `SDQ1` | Magic and version. A future format is `SDQ2`, and an old reader fails on the first comparison rather than misparsing three fields. |
| `code` | The six-digit pairing code, **generated by the desktop**. It keys the HMAC on the announcement and never travels on the wire itself. |
| `port` | The port the desktop is listening for the announcement on: `DISCOVERY_PORT`, 4011. |
| `nonce` | 16 random bytes, fresh per invite. Identical in role to the nonce a broadcast probe carries. |
| `addresses` | Every IPv4 address of the desktop a phone could send to, best first, at most `MAX_INVITE_ADDRESSES` (6). |

Loopback, unspecified, link-local (`169.254/16`), multicast and broadcast
addresses are dropped when the invite is built and rejected when one is parsed.
They appear in any real Windows adapter list and none of them is somewhere a
phone can send.

### 7.3 Why this character set

Every character above is in the QR alphanumeric set (`0-9`, `A-Z`, and
`` $%*+-./: `` plus space). An encoder packs those at 5.5 bits per character
instead of the 8 that byte mode costs. That is why the address separator is a
dash and not a comma, and why the nonce is unpadded base32 rather than base64
or hexadecimal: base64 needs lowercase and `/+=`, and hexadecimal would cost
six more characters.

A two-address invite is about 70 characters, which fits a version 4 symbol at
medium error correction. The modules stay large enough to read across a desk.

### 7.4 The invite is a probe delivered optically

The announcement the phone sends back is **byte for byte** the reply
`discovery::encode_announce` already builds for a broadcast probe (section
`discovery.rs`, magic `SDDS`, version 3), tagged with the same HMAC-SHA256
keyed by the same pairing code over the same body. The desktop verifies it with
`discovery::decode_announce` against the nonce and code the QR carried.

There is deliberately no second authentication path, and no new field is
trusted.

### 7.4.1 The two datagrams after it

The announcement is not the end of the exchange. A verified announcement is
followed by a **key offer** and a **key accept**, the same two datagrams on
both pairing paths, each an `SDDS` message tagged with the same HMAC keyed by
the same code and bound to the same nonce:

```text
  desktop                                             phone
    |  probe carrying nonce N, or the same N in a QR    |
    | ------------------------------------------------> |
    |  announce: name, port, HMAC(code; N, body)         |
    | <------------------------------------------------- |
    |  key offer:  PA, HMAC(code; N, 3, PA)              |
    | ------------------------------------------------> |
    |  key accept: PB, HMAC(code; N, 4, PA, PB)          |
    | <------------------------------------------------- |
```

`PA` and `PB` are ephemeral X25519 public keys, 32 bytes each. Both ends then
hold `X25519(a, PB) == X25519(b, PA)` and derive one master secret from it,
which is what every audio datagram of every later session is keyed from.

The code authenticates this exchange; it is **not** the key, and it could not
be: six digits is 19.93 bits with an offline verifier already on the wire.
[ADR-009](adr/ADR-009-audio-encryption.md) is written around that distinction.

Two extra datagrams rather than fields inside the probe and the announcement,
because the QR path has no probe on the wire at all: a public key carried in a
probe would never reach a phone that was given the invite optically. One
handshake serves both paths, and it costs one round trip per pairing.

### 7.5 What the threat model gains and loses

The code is now on the desktop's screen rather than the phone's. The one thing
that changes is that somebody who photographs or shoulder-surfs **the desktop's
screen** learns the code and could announce a device of their own in the
phone's place. That was already true of the phone-side code the user reads
aloud, and the invite is on screen for one pairing window rather than for a
whole session.

Everything else is unchanged:

- The code never reaches the wire. Only an HMAC keyed by it does.
- The nonce is fresh per invite, so a photograph of yesterday's screen does not
  authenticate today.
- Showing a new invite replaces the old one. Two live codes would be two ways
  in.
- The audio that follows is encrypted, under a key from the exchange in section
  7.4.1 and not from the code. Somebody who photographs the screen learns the
  code; they do not learn the key, because X25519 is not weakened by knowing
  the value that authenticated it. What the photograph buys is the chance to
  impersonate the phone **during that one pairing window**, which is the same
  thing it always bought.

---

## 8. Sonduit sealed audio (version 2)

Not part of Scream either. Defined in
`crates/sonduit-transport/src/sealed.rs`, decided in
[ADR-009](adr/ADR-009-audio-encryption.md).

Every audio datagram of a paired session is ChaCha20-Poly1305, keyed from the
handshake in section 7.4.1. The header is authenticated and not encrypted,
which is SRTP's arrangement and is deliberate: a receiver has to route a
datagram, pick a key by salt and reject a replay before it can afford to
authenticate anything.

```text
 0..4   magic "SDT1"
 4      version, 2 for sealed
 5      flags, as version 1
 6..8   packet counter, low 16 bits -- the sequence number, unchanged
 8..12  timestamp: frames elapsed on the sender's sample clock
12      sample rate marker
13      bits per sample
14      channel count
15      reserved, must be zero
16..18  channel mask
18..20  plaintext length in bytes
20..24  packet counter, high 32 bits
24..32  stream salt
32..    ciphertext, then the 16-byte Poly1305 tag
```

Bytes 0 to 20 keep the meaning they have in version 1, so anything reading a
sequence number or a format for telemetry needs one reader and not two. The
whole 32-byte header is the AEAD's associated data.

| Question | Answer |
| --- | --- |
| Nonce | The 48-bit packet counter, little-endian in the low 8 bytes of the 12. Never random, never reused, and it does not wrap: 2^48 packets at 6 ms is 53 million years |
| Why not the sequence number | 16 bits wraps every 393 seconds, which is an ordinary event in a real session, and a repeated nonce hands over the keystream and the authenticator's key together |
| Counter restart | Answered by making the key fresh instead: 8 random bytes of stream salt per stream, in bytes 24..32, feeding `HKDF-SHA256(salt, master, "sonduit-audio-v1")` |
| Replay | A 256-packet sliding window in `Opener`, RFC 4303 style, plus the last 8 retired salts, which a genuine salt never repeats |
| Overhead | 48 bytes rather than 20, so 1200 on the wire against 1172. Still far below any MTU |

### 8.1 Version 1 and version 2 do not mix

The version byte is the whole compatibility story, and it is checked before a
byte of payload is looked at.

| Meeting | What happens |
| --- | --- |
| Sealed packet, receiver with the key | Opened and played |
| Sealed packet, receiver with no key | Refused. Decoding ciphertext as PCM would be a full-scale noise burst |
| Cleartext packet, receiver with the key | Refused. **This is the downgrade defence and it is not optional:** a keyed receiver that still accepted version 1 would let an attacker simply send version 1 |
| Scream packet, receiver with the key | Refused, for the same reason. A wire with no version field is not a way around the check |
| Anything at all, receiver with no key | The version 1 rules, unchanged |

A receiver holds a key from the moment it pairs and until the user asks for a
new pairing code, which discards it. That is the one way back to a receiver
that will accept an unencrypted sender, and an unmodified Scream driver is
exactly such a sender: its five-byte header has no version field and nowhere to
put a tag, so there is no sealed Scream and there cannot be one.

### 8.2 The feedback report is sealed too

`SDFB` version 2: the magic, the version, a reserved byte, an 8-byte counter,
the stream salt, then the sealed version 1 body and its tag -- 72 bytes against
34. The key is derived with a different label (`"sonduit-feedback-v1"`), so a
report can never be replayed into the audio path or the reverse. A keyed sender
refuses a cleartext report for the same reason a keyed receiver refuses
cleartext audio: the report drives the loss figure, the buffer depth and the
round trip the user is shown, and `FEEDBACK_TIMEOUT_MS` means a forged one can
keep a dead session looking alive.

