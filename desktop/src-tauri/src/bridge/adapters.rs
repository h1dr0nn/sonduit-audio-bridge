//! Finding the tethered phone without asking the user for an address.
//!
//! # Why this exists
//!
//! Over USB the phone is a DHCP server and the PC is its client, so the phone
//! is the gateway on that interface. Reading the gateway off the tethered
//! adapter gives its address directly, with no broadcast and no guessing.
//!
//! # Why the address is never hardcoded
//!
//! 192.168.42.129 is what AOSP uses and what most guides quote, but it is a
//! default, not a guarantee: `docs/research/usb-transport.md` records OEMs
//! shipping other ranges. Hardcoding it produces a product that works on the
//! developer's phone and silently fails on somebody else's.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// A network interface that looks like a tethered phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TetherAdapter {
    /// Adapter description, as Windows reports it.
    pub description: String,
    /// The gateway on this interface, which is the phone.
    pub gateway: IpAddr,
    /// This machine's own address on the same interface.
    ///
    /// Needed to bind the sending socket, which is the entire difference
    /// between the Wi-Fi path and the USB one.
    pub local: IpAddr,
}

impl TetherAdapter {
    /// Where to send audio, given the port the receiver announced.
    #[must_use]
    pub fn target(&self, port: u16) -> SocketAddr {
        SocketAddr::new(self.gateway, port)
    }

    /// What to bind locally so the datagrams leave by this interface.
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        SocketAddr::new(self.local, 0)
    }
}

/// Protocol names that only appear in a tethering driver.
///
/// Matched as whole words. As substrings, `ncm` alone would match anything
/// containing those three letters, and a display adapter is not a phone.
/// `ndis` rather than `rndis`, because Windows writes the AOSP driver as
/// "Remote NDIS" with a space, which does not contain "rndis" at all. That
/// mistake made this function match nothing at all on the most common device
/// there is.
const TETHER_TOKENS: [&str; 3] = ["ndis", "rndis", "ncm"];

/// Phrases that identify a tether, matched anywhere in the description.
///
/// These are multi-word and specific enough that a substring match is safe.
const TETHER_PHRASES: [&str; 3] = ["usb ethernet", "internet sharing", "tethering"];

/// Whether an adapter description looks like a tethered phone.
///
/// Matching on the description is unlovely, but the alternative is matching on
/// the address range, which is exactly the assumption this module exists to
/// avoid. Case-insensitive: the description is a manufacturer string and its
/// capitalisation is whatever the driver author chose.
#[must_use]
pub fn looks_like_tether(description: &str) -> bool {
    let lowered = description.to_lowercase();

    if TETHER_PHRASES.iter().any(|phrase| lowered.contains(phrase)) {
        return true;
    }

    lowered
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| TETHER_TOKENS.contains(&token))
}

/// Whether an address is in the range Android hands out when tethering.
///
/// Used to rank candidates, never to find them: an adapter in this range is
/// more likely to be the phone, but an adapter outside it is not disqualified.
#[must_use]
pub fn in_android_tether_range(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets[0] == 192 && octets[1] == 168 && octets[2] == 42
        }
        IpAddr::V6(_) => false,
    }
}

/// Rank candidates so the most likely tether comes first.
///
/// Pure, so the ordering can be tested without a phone plugged in.
#[must_use]
pub fn rank(mut candidates: Vec<TetherAdapter>) -> Vec<TetherAdapter> {
    candidates.sort_by_key(|adapter| {
        let named = u8::from(!looks_like_tether(&adapter.description));
        let ranged = u8::from(!in_android_tether_range(adapter.gateway));
        // Both signals agreeing beats either alone, and a named adapter beats
        // one that merely sits in the right range: a home network could use
        // 192.168.42/24 by coincidence, but nothing else is called RNDIS.
        (named, ranged)
    });
    candidates
}

/// Ask Windows for the IPv4 adapter list, returning the raw buffer.
///
/// Shared by the two walks below rather than written twice: the retry dance
/// around a buffer that can grow between calls is the part that is easy to get
/// subtly wrong, and one copy of it is one copy to get right.
#[cfg(windows)]
fn adapter_list() -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST,
        GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    // GAA_FLAG_INCLUDE_GATEWAYS is not optional and not obvious: without it
    // FirstGatewayAddress is null on every adapter and the whole enumeration
    // silently returns nothing. Found by running this against a machine whose
    // gateway Windows itself was happy to report.
    let flags = GAA_FLAG_INCLUDE_GATEWAYS
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;

    // Called twice: once to learn the size, once to fill it. Microsoft
    // recommends starting at 15 KB, which usually succeeds outright.
    let mut size = 15_000_u32;
    let mut buffer = vec![0_u8; size as usize];

    for _ in 0..3 {
        // SAFETY: buffer holds at least `size` bytes, which is the contract
        // this call requires, and `size` is updated by the call itself.
        let result = WIN32_ERROR(unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET.0),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size,
            )
        });

        if result == ERROR_SUCCESS {
            return Ok(buffer);
        }
        if result != ERROR_BUFFER_OVERFLOW {
            return Err(format!("GetAdaptersAddresses failed: {}", result.0));
        }
        buffer.resize(size as usize, 0);
    }

    Err("the adapter list kept growing between calls".to_string())
}

/// Every IPv4 address this machine holds that a phone could send to.
///
/// This is what goes in the pairing QR code. It deliberately does not require
/// a gateway, unlike [`enumerate`]: an address is worth offering whether or
/// not the interface it sits on can route anywhere, because the phone only has
/// to reach this machine and not the internet.
///
/// Tether-looking adapters come first. The invite carries a bounded number of
/// addresses, and over USB the phone is certainly on that link, so it must not
/// be the one that gets dropped off the end of the list.
///
/// # Errors
/// Returns a description of the failure when Windows refuses to enumerate.
#[cfg(windows)]
pub fn local_ipv4() -> Result<Vec<Ipv4Addr>, String> {
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    let buffer = adapter_list()?;
    let mut preferred = Vec::new();
    let mut rest = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();

    while !adapter.is_null() {
        // SAFETY: the pointer came from the list Windows just filled, and the
        // loop stops at the null terminator it wrote.
        let current = unsafe { &*adapter };
        adapter = current.Next;

        // A disconnected adapter keeps its last address. Putting that in the
        // QR would send the phone at an address nothing answers on, and the
        // user would be told the pairing timed out with no way to know why.
        if current.OperStatus != IfOperStatusUp {
            continue;
        }

        // SAFETY: Description is a null-terminated wide string in the buffer.
        let description = unsafe { current.Description.to_string() }.unwrap_or_default();
        let bucket = if looks_like_tether(&description) {
            &mut preferred
        } else {
            &mut rest
        };

        for address in all_unicast(current.FirstUnicastAddress) {
            // Filtered here as well as in the invite, because this list is
            // shown to the user beside the QR code. The loopback interface is
            // always up and always has 127.0.0.1, and printing it would tell
            // the user their phone can reach an address it never can.
            if let IpAddr::V4(ip) = address {
                if sonduit_transport::invite::is_reachable(ip) {
                    bucket.push(ip);
                }
            }
        }
    }

    preferred.append(&mut rest);
    Ok(preferred)
}

/// Enumerate interfaces that could be a tethered phone, best first.
///
/// # Errors
/// Returns a description of the failure when Windows refuses to enumerate.
#[cfg(windows)]
pub fn enumerate() -> Result<Vec<TetherAdapter>, String> {
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;

    let buffer = adapter_list()?;
    let mut found = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();

    while !adapter.is_null() {
        // SAFETY: the pointer came from the list Windows just filled, and the
        // loop stops at the null terminator it wrote.
        let current = unsafe { &*adapter };

        // SAFETY: Description is a null-terminated wide string in the buffer.
        let description = unsafe { current.Description.to_string() }.unwrap_or_default();

        let gateway = first_gateway(current.FirstGatewayAddress);
        let local = first_unicast(current.FirstUnicastAddress);

        if let (Some(gateway), Some(local)) = (gateway, local) {
            found.push(TetherAdapter {
                description,
                gateway,
                local,
            });
        }

        adapter = current.Next;
    }

    // Everything with a gateway is a candidate; ranking decides which is the
    // phone. Returning only the matches would leave a user with an
    // unrecognised adapter no way through at all.
    Ok(rank(found))
}

/// The first IPv4 gateway on an adapter, which over USB is the phone.
#[cfg(windows)]
fn first_gateway(
    mut node: *const windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_GATEWAY_ADDRESS_LH,
) -> Option<IpAddr> {
    while !node.is_null() {
        // SAFETY: the pointer came from an adapter Windows filled, and the
        // loop stops at its null terminator.
        let current = unsafe { &*node };
        if let Some(address) = read_socket_address(current.Address) {
            return Some(address);
        }
        node = current.Next;
    }
    None
}

/// The first IPv4 address this machine holds on an adapter.
#[cfg(windows)]
fn first_unicast(
    mut node: *const windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_UNICAST_ADDRESS_LH,
) -> Option<IpAddr> {
    while !node.is_null() {
        // SAFETY: as above.
        let current = unsafe { &*node };
        if let Some(address) = read_socket_address(current.Address) {
            return Some(address);
        }
        node = current.Next;
    }
    None
}

/// Every IPv4 address on an adapter, not just the first.
///
/// An interface can hold several, and the one the phone shares a subnet with
/// is not necessarily the one Windows lists first.
#[cfg(windows)]
fn all_unicast(
    mut node: *const windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_UNICAST_ADDRESS_LH,
) -> Vec<IpAddr> {
    let mut found = Vec::new();
    while !node.is_null() {
        // SAFETY: as above.
        let current = unsafe { &*node };
        if let Some(address) = read_socket_address(current.Address) {
            found.push(address);
        }
        node = current.Next;
    }
    found
}

/// Read an IPv4 address out of a `SOCKET_ADDRESS`, if that is what it holds.
#[cfg(windows)]
fn read_socket_address(
    address: windows::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Option<IpAddr> {
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    if address.lpSockaddr.is_null() {
        return None;
    }

    // SAFETY: sa_family is the first field of every sockaddr variant, so it is
    // readable before the variant is known.
    let family = unsafe { (*address.lpSockaddr).sa_family };
    if family != AF_INET {
        return None;
    }

    // SAFETY: the family says this is a SOCKADDR_IN.
    let inet = unsafe { &*address.lpSockaddr.cast::<SOCKADDR_IN>() };
    // SAFETY: S_un is a union of equivalent representations of the same four
    // bytes; the byte form is the one that needs no endianness handling.
    let octets = unsafe { inet.sin_addr.S_un.S_un_b };
    Some(IpAddr::V4(Ipv4Addr::new(
        octets.s_b1,
        octets.s_b2,
        octets.s_b3,
        octets.s_b4,
    )))
}

/// Off Windows there is no adapter list to read.
///
/// # Errors
/// Always returns a message saying so.
#[cfg(not(windows))]
pub fn enumerate() -> Result<Vec<TetherAdapter>, String> {
    Err("adapter enumeration is implemented for Windows only".to_string())
}

/// Off Windows there is no adapter list to read.
///
/// # Errors
/// Always returns a message saying so.
#[cfg(not(windows))]
pub fn local_ipv4() -> Result<Vec<Ipv4Addr>, String> {
    Err("adapter enumeration is implemented for Windows only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(description: &str, gateway: [u8; 4]) -> TetherAdapter {
        TetherAdapter {
            description: description.to_string(),
            gateway: IpAddr::V4(Ipv4Addr::from(gateway)),
            local: IpAddr::V4(Ipv4Addr::new(192, 168, 42, 100)),
        }
    }

    #[test]
    fn the_drivers_a_tethered_phone_installs_are_recognised() {
        // These are the strings Windows actually shows. RNDIS on older and
        // Samsung devices, NCM on newer ones.
        for description in [
            "Remote NDIS based Internet Sharing Device",
            "Samsung Mobile USB Remote NDIS Network Device",
            "USB Ethernet/RNDIS Gadget",
            "NCM Network Adapter",
            "Android USB Ethernet/RNDIS",
        ] {
            assert!(looks_like_tether(description), "missed {description:?}");
        }
    }

    #[test]
    fn ordinary_adapters_are_not_mistaken_for_a_phone() {
        for description in [
            "Intel(R) Wi-Fi 6 AX201 160MHz",
            "Realtek PCIe GbE Family Controller",
            "Hyper-V Virtual Ethernet Adapter",
        ] {
            assert!(!looks_like_tether(description), "matched {description:?}");
        }
    }

    #[test]
    fn a_word_that_merely_contains_a_hint_is_not_a_phone() {
        // Substring matching on three letters is how a display adapter ends up
        // being offered as a tethered phone.
        assert!(!looks_like_tether("Syncmaster Display Adapter"));
        assert!(!looks_like_tether("Broadcom NetXtreme Gigabit"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        // The description is a manufacturer string; its capitalisation is
        // whatever the driver author felt like.
        assert!(looks_like_tether("REMOTE NDIS DEVICE"));
        assert!(looks_like_tether("remote ndis device"));
    }

    #[test]
    fn the_android_range_is_recognised_without_being_required() {
        assert!(in_android_tether_range(IpAddr::V4(Ipv4Addr::new(
            192, 168, 42, 129
        ))));
        assert!(!in_android_tether_range(IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
    }

    #[test]
    fn a_named_tether_outranks_one_that_is_merely_in_the_right_range() {
        // A home network can use 192.168.42/24 by coincidence. Nothing else is
        // called RNDIS.
        let ranked = rank(vec![
            adapter("Realtek PCIe GbE", [192, 168, 42, 1]),
            adapter("Remote NDIS based Internet Sharing Device", [10, 0, 0, 1]),
        ]);

        assert!(looks_like_tether(&ranked[0].description));
    }

    #[test]
    fn both_signals_agreeing_outranks_either_alone() {
        let ranked = rank(vec![
            adapter("Remote NDIS based Internet Sharing Device", [10, 0, 0, 1]),
            adapter("Realtek PCIe GbE", [192, 168, 42, 1]),
            adapter("USB Ethernet/RNDIS Gadget", [192, 168, 42, 129]),
        ]);

        assert_eq!(ranked[0].description, "USB Ethernet/RNDIS Gadget");
    }

    #[test]
    fn an_unrecognised_adapter_is_still_offered_rather_than_dropped() {
        // An OEM this list has never seen must not leave the user with nothing
        // to select.
        let ranked = rank(vec![adapter("Some Vendor Gadget", [10, 5, 5, 1])]);
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn the_target_takes_the_gateway_and_the_announced_port() {
        let adapter = adapter("Remote NDIS", [192, 168, 42, 129]);
        assert_eq!(
            adapter.target(4010),
            "192.168.42.129:4010".parse::<SocketAddr>().unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_local_address_list_offers_nothing_a_phone_cannot_send_to() {
        // Runs against the real adapter list, because the bug this guards
        // against is not in the filtering logic but in what Windows actually
        // hands back: a disconnected adapter keeps its last address, and a
        // machine with no route still has 127.0.0.1 and often a 169.254 one.
        // Any of those in a QR code sends the phone at an address that will
        // never answer.
        let addresses = local_ipv4().expect("Windows must be able to list its own adapters");
        println!("local IPv4 addresses offered to a phone: {addresses:?}");

        for address in &addresses {
            assert!(!address.is_loopback(), "{address} is loopback");
            assert!(!address.is_unspecified(), "{address} is unspecified");
            assert!(!address.is_link_local(), "{address} is link-local");
            assert!(!address.is_multicast(), "{address} is multicast");
        }
    }

    #[test]
    fn the_bind_address_is_this_machines_side_of_the_link() {
        // Binding the phone's address would fail; binding 0.0.0.0 would let
        // the routing table send over Wi-Fi instead, which is the bug this
        // whole module exists to prevent.
        let adapter = adapter("Remote NDIS", [192, 168, 42, 129]);
        assert_eq!(adapter.bind().ip(), adapter.local);
        assert_eq!(adapter.bind().port(), 0, "the source port does not matter");
    }
}
