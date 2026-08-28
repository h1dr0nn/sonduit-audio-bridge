//! Which link a session is on.
//!
//! # One answer, two consumers
//!
//! "Wi-Fi or USB" used to be answered twice. The wire asked the routing table
//! which interface reaches the target and put the answer in the packet header
//! as `FLAG_WIRED_LINK`; the telemetry panel tested the target against
//! `192.168.42/24` and printed whichever word that suggested. On a phone that
//! tethers on `10.114.89.x` the two disagreed, and the panel said "Wi-Fi"
//! while the audio was going over the cable.
//!
//! Everything now goes through [`LinkKind`], established by [`observe`] once
//! per route and carried on the [`Link`] the send loop holds. The header flag
//! and the label the user reads are two renderings of that one value, so they
//! cannot drift.
//!
//! # Why the routing table and not the address
//!
//! USB tethering has no reserved range. AOSP's `192.168.42/24` is a default
//! that OEMs override, and on Android 16 the range usually lands somewhere in
//! `10/8` (see ADR-004). The only thing that knows which wire a datagram will
//! leave by is the routing table, so that is what gets asked: connecting a
//! throwaway UDP socket sends nothing and only makes the kernel pick a source
//! address, and the adapter list can name the interface that address belongs
//! to.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

use super::adapters::{self, TetherAdapter};

/// What kind of link a route uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A tethered phone: datagrams leave over the USB adapter.
    Wired,
    /// Anything else that reaches one host: Wi-Fi, Ethernet, a VPN.
    Wireless,
    /// A group address. Nobody in particular is on the other end, so there is
    /// no link to describe and nothing to migrate to or from.
    Multicast,
}

impl LinkKind {
    /// The word the UI renders, and the value the frontend switches on.
    ///
    /// Deliberately the same three strings the panel has always shown, so the
    /// translation keys and the status menu did not have to change meaning
    /// when the derivation did.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Wired => "usb",
            Self::Wireless => "wifi",
            Self::Multicast => "multicast",
        }
    }

    /// Whether the packet header should claim a wired link.
    ///
    /// False on any doubt, which is what multicast and an unrecognised
    /// interface both give. Claiming a wired link that is not one makes the
    /// receiver hold ten milliseconds against Wi-Fi's jitter, which underruns;
    /// the reverse only costs latency.
    #[must_use]
    pub const fn is_wired(self) -> bool {
        matches!(self, Self::Wired)
    }

    /// Round-trip through a byte, for the atomic the send loop publishes.
    const fn code(self) -> u8 {
        match self {
            Self::Wired => 1,
            Self::Wireless => 2,
            Self::Multicast => 3,
        }
    }

    /// The inverse of [`Self::code`]. Anything unrecognised is multicast,
    /// which is the value that migrates nowhere.
    const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Wired,
            2 => Self::Wireless,
            _ => Self::Multicast,
        }
    }
}

/// A way to reach the peer: what to bind, where to send, and over what.
///
/// A plan rather than a connection. Binding is what [`Link`] adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Where the audio goes.
    pub target: SocketAddr,
    /// What to bind locally, which is what selects the interface. ADR-004:
    /// with both links up the routing table may pick the wrong one, so the
    /// local address is not left to it.
    pub bind: SocketAddr,
    /// Which link this route uses.
    pub kind: LinkKind,
}

impl Route {
    /// A route that lets the routing table choose the interface.
    ///
    /// What a session started with a typed address gets: the user named a
    /// destination and nothing about how to get there.
    #[must_use]
    pub fn unbound(target: SocketAddr, kind: LinkKind) -> Self {
        Self {
            target,
            bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            kind,
        }
    }

    /// The route over a tethered adapter to the phone that is its gateway.
    #[must_use]
    pub fn over(adapter: &TetherAdapter, port: u16) -> Self {
        Self {
            target: adapter.target(port),
            bind: adapter.bind(),
            kind: LinkKind::Wired,
        }
    }
}

/// A bound socket on a route: everything the send loop needs to send.
#[derive(Debug)]
pub struct Link {
    /// The socket, already bound and already non-blocking.
    pub socket: UdpSocket,
    /// The route it was bound for.
    pub route: Route,
}

impl Link {
    /// Bind a socket for `route`.
    ///
    /// Non-blocking is set here rather than left to the send loop, because
    /// that loop reads the same socket for the receiver's reports and a
    /// blocking read there waits forever the moment there is nothing to read.
    /// A socket handed over mid-session must arrive with that already true.
    ///
    /// # Errors
    /// Returns the socket error, which on a route whose interface has just
    /// disappeared is the ordinary outcome rather than a fault.
    pub fn bind(route: Route) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(route.bind)?;
        socket.set_nonblocking(true)?;
        if route.target.ip().is_multicast() {
            socket.set_multicast_ttl_v4(1)?;
        } else if matches!(route.target.ip(), IpAddr::V4(ip) if ip.is_broadcast()) {
            socket.set_broadcast(true)?;
        }
        Ok(Self { socket, route })
    }
}

/// The hand-off between the link watcher and the send loop.
///
/// # Why the watcher does the binding
///
/// Binding a socket is a syscall, and enumerating adapters to decide *which*
/// socket is several. Neither belongs on the thread that has to hand a packet
/// to the network every six milliseconds. So the watcher does all of it and
/// leaves a finished, bound, non-blocking socket here; the send loop's whole
/// share of a migration is taking it out of an uncontended mutex and replacing
/// two locals, which is why the migration costs no audio at all.
///
/// # Why the send loop's check is not a lock
///
/// It runs once per capture block. `ready` is a relaxed load and is false
/// every time but the one, so the mutex is touched only when there is
/// something in it.
#[derive(Debug, Default)]
pub struct LinkSwitch {
    /// A link the watcher has decided on. The send loop takes it at once.
    pending: Mutex<Option<Link>>,
    /// Whether `pending` holds anything, so the common case is one load.
    pending_ready: AtomicBool,
    /// A link the send loop may retreat to on evidence only it can see fast
    /// enough: the current route refusing datagrams. Pre-bound so that
    /// retreat costs nothing at the moment it is needed.
    standby: Mutex<Option<Link>>,
    /// Whether `standby` holds anything.
    standby_ready: AtomicBool,
    /// The route the send loop is actually on. Written by it, read by the
    /// watcher, which cannot otherwise know that a retreat has happened.
    live: Mutex<Option<Route>>,
    /// [`LinkKind::code`] of the live route, so the common read is a load.
    live_kind: AtomicU8,
    /// Set when the send loop takes the standby, so the watcher stops
    /// sleeping and re-evaluates instead of waiting out its poll.
    retreated: AtomicBool,
}

impl LinkSwitch {
    /// A switch for a session that starts on `route`.
    #[must_use]
    pub fn new(route: Route) -> Self {
        let switch = Self::default();
        switch.set_live(route);
        switch
    }

    /// Publish the route the send loop is now on.
    pub fn set_live(&self, route: Route) {
        self.live_kind.store(route.kind.code(), Ordering::Relaxed);
        if let Ok(mut slot) = self.live.lock() {
            *slot = Some(route);
        }
    }

    /// The route the send loop is on, if it has published one.
    #[must_use]
    pub fn live(&self) -> Option<Route> {
        self.live.lock().ok().and_then(|slot| slot.clone())
    }

    /// Which link the send loop is on, without taking the lock.
    #[must_use]
    pub fn live_kind(&self) -> LinkKind {
        LinkKind::from_code(self.live_kind.load(Ordering::Relaxed))
    }

    /// Hand a bound link to the send loop, to be adopted immediately.
    ///
    /// A previous offer that has not been taken is replaced: the newer
    /// decision was made with newer evidence.
    pub fn offer(&self, link: Link) {
        if let Ok(mut slot) = self.pending.lock() {
            *slot = Some(link);
            // Released after the slot is filled, so the send loop cannot see
            // the flag before the link is there.
            self.pending_ready.store(true, Ordering::Release);
        }
    }

    /// Take whatever the watcher offered, if anything.
    ///
    /// `try_lock`, never `lock`: this runs on the send loop, and the one
    /// moment the watcher holds the mutex is the one moment that loop must not
    /// wait. Missing an offer costs one capture block of delay.
    #[must_use]
    pub fn take_offer(&self) -> Option<Link> {
        if !self.pending_ready.load(Ordering::Acquire) {
            return None;
        }
        let mut slot = self.pending.try_lock().ok()?;
        let taken = slot.take();
        if taken.is_some() {
            self.pending_ready.store(false, Ordering::Release);
        }
        taken
    }

    /// Pre-bind a retreat the send loop may take on its own.
    pub fn arm(&self, link: Link) {
        if let Ok(mut slot) = self.standby.lock() {
            *slot = Some(link);
            self.standby_ready.store(true, Ordering::Release);
        }
    }

    /// Whether a retreat is ready to be taken.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.standby_ready.load(Ordering::Acquire)
    }

    /// Take the armed retreat, disarming it.
    ///
    /// Taking it also raises [`Self::retreated`], because the watcher has to
    /// re-arm and re-evaluate rather than discover the change a poll later.
    #[must_use]
    pub fn take_retreat(&self) -> Option<Link> {
        if !self.standby_ready.load(Ordering::Acquire) {
            return None;
        }
        let mut slot = self.standby.try_lock().ok()?;
        let taken = slot.take();
        if taken.is_some() {
            self.standby_ready.store(false, Ordering::Release);
            self.retreated.store(true, Ordering::Release);
        }
        taken
    }

    /// Throw away an armed retreat that is no longer the right one.
    pub fn disarm(&self) {
        if let Ok(mut slot) = self.standby.lock() {
            *slot = None;
            self.standby_ready.store(false, Ordering::Release);
        }
    }

    /// Whether the send loop has retreated since this was last asked.
    ///
    /// Clears the flag, so a watcher that has already reacted does not react
    /// again to the same event.
    #[must_use]
    pub fn took_retreat(&self) -> bool {
        self.retreated.swap(false, Ordering::AcqRel)
    }
}

/// The source address the routing table would use to reach `target`.
///
/// Connecting a UDP socket sends nothing. It only asks the kernel to pick a
/// source, and the answer names the interface. `None` when there is no route
/// at all, which is itself the answer worth having: the link is gone.
#[must_use]
pub fn route_source(target: SocketAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    probe.connect(target).ok()?;
    Some(probe.local_addr().ok()?.ip())
}

/// Decide the link kind from a target, a source address and the adapter list.
///
/// Pure, so the rule can be tested against a synthetic adapter list on a
/// machine with no phone attached. Everything platform-specific is in the two
/// arguments.
///
/// A source address that belongs to an adapter whose description names a
/// tethering protocol is the only thing that produces [`LinkKind::Wired`].
/// Both halves are required: the description alone would match an adapter the
/// datagrams do not leave by, and the address alone cannot tell a tether from
/// any other private network.
#[must_use]
pub fn classify(
    target: SocketAddr,
    source: Option<IpAddr>,
    adapters: &[TetherAdapter],
) -> LinkKind {
    if target.ip().is_multicast() {
        return LinkKind::Multicast;
    }
    if matches!(target.ip(), IpAddr::V4(ip) if ip.is_broadcast()) {
        return LinkKind::Multicast;
    }

    let Some(source) = source else {
        return LinkKind::Wireless;
    };

    let wired = adapters.iter().any(|adapter| {
        adapter.local == source && adapters::looks_like_tether(&adapter.description)
    });

    if wired {
        LinkKind::Wired
    } else {
        LinkKind::Wireless
    }
}

/// Ask the machine which link reaches `target`.
///
/// The I/O half of [`classify`]: one throwaway socket and one walk of the
/// adapter list. Cheap enough to do when a session starts or when a route
/// changes, and far too expensive to do per packet, so nothing on the send
/// path calls it.
#[must_use]
pub fn observe(target: SocketAddr) -> LinkKind {
    if target.ip().is_multicast() {
        return LinkKind::Multicast;
    }
    let adapters = adapters::enumerate().unwrap_or_default();
    classify(target, route_source(target), &adapters)
}

/// Which link a session bound to `bind` and sending to `target` is on.
///
/// The single entry point every caller uses. When the local address is left
/// unspecified the routing table is asked; when it is explicit there is
/// nothing to ask, because that address is the interface.
#[must_use]
pub fn for_route(target: SocketAddr, bind: SocketAddr) -> LinkKind {
    if target.ip().is_multicast() {
        return LinkKind::Multicast;
    }
    let adapters = adapters::enumerate().unwrap_or_default();
    let source = if bind.ip().is_unspecified() {
        route_source(target)
    } else {
        Some(bind.ip())
    };
    classify(target, source, &adapters)
}

/// Whether an address is the far end of a tethered adapter on this machine.
///
/// The phone's own address on the tether segment, in other words: over USB it
/// is the gateway by construction, because the phone is the DHCP server. Used
/// to tell a broadcast reply that came over the cable from one that did not,
/// without opening a socket per reply to ask the routing table.
#[must_use]
pub fn is_tether_gateway(address: IpAddr, adapters: &[TetherAdapter]) -> bool {
    adapters.iter().any(|adapter| {
        adapter.gateway == address && adapters::looks_like_tether(&adapter.description)
    })
}

/// Whether a route still has the interface it was bound to.
///
/// Only meaningful for a wired route, which is the one that disappears: the
/// adapter goes when the cable does. For anything else the answer comes from
/// the routing table, which is the same question [`route_source`] answers.
#[must_use]
pub fn route_alive(route: &Route, adapters: &[TetherAdapter]) -> bool {
    match route.kind {
        LinkKind::Wired => adapters.iter().any(|adapter| {
            adapter.local == route.bind.ip() && adapters::looks_like_tether(&adapter.description)
        }),
        // A multicast route needs nothing on the far end to exist, so it is
        // never the reason to move.
        LinkKind::Multicast => true,
        LinkKind::Wireless => route_source(route.target).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(description: &str, local: [u8; 4], gateway: [u8; 4]) -> TetherAdapter {
        TetherAdapter {
            description: description.to_string(),
            gateway: IpAddr::V4(Ipv4Addr::from(gateway)),
            local: IpAddr::V4(Ipv4Addr::from(local)),
        }
    }

    fn tether() -> TetherAdapter {
        // The machine this was written on: a real tether, nowhere near the
        // 192.168.42/24 the old guess tested for.
        adapter("UsbNcm Host Device", [10, 114, 89, 252], [10, 114, 89, 244])
    }

    fn wifi() -> TetherAdapter {
        adapter(
            "Intel(R) Wi-Fi 6 AX201 160MHz",
            [192, 168, 1, 20],
            [192, 168, 1, 1],
        )
    }

    fn at(address: &str) -> SocketAddr {
        address.parse().expect("a literal address")
    }

    fn ip(address: &str) -> IpAddr {
        address.parse().expect("a literal address")
    }

    #[test]
    fn a_tether_outside_the_android_range_is_still_wired() {
        // The bug. The phone tethers on 10.114.89.x, the panel tested against
        // 192.168.42/24, and the user was told Wi-Fi while the audio went over
        // the cable.
        let kind = classify(
            at("10.114.89.244:4010"),
            Some(ip("10.114.89.252")),
            &[tether(), wifi()],
        );
        assert_eq!(kind, LinkKind::Wired);
        assert_eq!(kind.label(), "usb");
        assert!(kind.is_wired());
    }

    #[test]
    fn an_address_in_the_android_range_reached_over_wifi_is_not_wired() {
        // The mirror of the same mistake: a home network that happens to use
        // 192.168.42/24 is not a phone, and the old guess called it USB.
        let lan = adapter("Realtek PCIe GbE", [192, 168, 42, 20], [192, 168, 42, 1]);
        let kind = classify(at("192.168.42.129:4010"), Some(ip("192.168.42.20")), &[lan]);
        assert_eq!(kind, LinkKind::Wireless);
        assert_eq!(kind.label(), "wifi");
    }

    #[test]
    fn the_source_address_has_to_belong_to_the_tether_and_not_merely_exist() {
        // A tether adapter is present, but the route to this target leaves by
        // Wi-Fi. Matching on the presence of the adapter rather than on the
        // address the kernel picked would send the receiver a wired claim for
        // a stream going over the radio.
        let kind = classify(
            at("192.168.1.50:4010"),
            Some(ip("192.168.1.20")),
            &[tether(), wifi()],
        );
        assert_eq!(kind, LinkKind::Wireless);
    }

    #[test]
    fn no_route_is_wireless_rather_than_wired() {
        // False on any doubt. A wired claim makes the receiver hold ten
        // milliseconds against Wi-Fi's jitter, which underruns; the reverse
        // only costs latency.
        assert_eq!(
            classify(at("10.114.89.244:4010"), None, &[tether()]),
            LinkKind::Wireless
        );
    }

    #[test]
    fn a_group_address_is_multicast_and_not_guessed_at() {
        assert_eq!(
            classify(at("239.255.77.77:4010"), None, &[]),
            LinkKind::Multicast
        );
        assert_eq!(
            classify(at("255.255.255.255:4010"), None, &[]),
            LinkKind::Multicast
        );
        assert!(!LinkKind::Multicast.is_wired());
    }

    #[test]
    fn an_adapter_that_holds_the_address_but_is_not_a_tether_is_not_wired() {
        // The description is half the evidence. Without it any interface the
        // kernel happened to pick would be reported as a cable.
        let hyperv = adapter(
            "Hyper-V Virtual Ethernet Adapter",
            [172, 20, 0, 1],
            [172, 20, 0, 254],
        );
        assert_eq!(
            classify(at("172.20.0.254:4010"), Some(ip("172.20.0.1")), &[hyperv]),
            LinkKind::Wireless
        );
    }

    #[test]
    fn the_phone_on_the_far_end_of_a_cable_is_recognised_by_its_gateway() {
        assert!(is_tether_gateway(ip("10.114.89.244"), &[tether(), wifi()]));
        // The router on the wireless network is a gateway too, and it is not
        // a phone. Matching every gateway would make every reply look wired.
        assert!(!is_tether_gateway(ip("192.168.1.1"), &[tether(), wifi()]));
        assert!(!is_tether_gateway(ip("192.168.1.42"), &[tether(), wifi()]));
    }

    #[test]
    fn a_wired_route_dies_with_its_adapter() {
        let route = Route::over(&tether(), 4010);
        assert!(route_alive(&route, &[tether(), wifi()]));
        assert!(
            !route_alive(&route, &[wifi()]),
            "the cable is out and the route is still being called alive"
        );
    }

    #[test]
    fn the_route_over_an_adapter_binds_this_side_and_targets_the_phone() {
        // Binding the gateway would fail; binding nothing would let the
        // routing table send over Wi-Fi instead, which is the whole of ADR-004
        // consequence 2.
        let route = Route::over(&tether(), 4010);
        assert_eq!(route.target, at("10.114.89.244:4010"));
        assert_eq!(route.bind.ip(), ip("10.114.89.252"));
        assert_eq!(route.bind.port(), 0, "the source port does not matter");
        assert_eq!(route.kind, LinkKind::Wired);
    }

    #[test]
    fn the_switch_reports_the_route_the_send_loop_published() {
        let switch = LinkSwitch::new(Route::unbound(at("192.168.1.5:4010"), LinkKind::Wireless));
        assert_eq!(switch.live_kind(), LinkKind::Wireless);

        switch.set_live(Route::over(&tether(), 4010));
        assert_eq!(switch.live_kind(), LinkKind::Wired);
        assert_eq!(
            switch.live().map(|route| route.target),
            Some(at("10.114.89.244:4010"))
        );
    }

    #[test]
    fn nothing_is_taken_from_a_switch_nobody_has_offered_to() {
        let switch = LinkSwitch::new(Route::unbound(at("192.168.1.5:4010"), LinkKind::Wireless));
        assert!(switch.take_offer().is_none());
        assert!(switch.take_retreat().is_none());
        assert!(!switch.armed());
        assert!(!switch.took_retreat());
    }

    #[test]
    fn an_offer_is_taken_once_and_only_once() {
        let switch = LinkSwitch::new(Route::unbound(at("127.0.0.1:4010"), LinkKind::Wireless));
        let route = Route::unbound(at("127.0.0.1:4010"), LinkKind::Wireless);
        switch.offer(Link::bind(route).expect("an ephemeral loopback socket"));

        assert!(switch.take_offer().is_some());
        assert!(
            switch.take_offer().is_none(),
            "the same migration was handed over twice"
        );
    }

    #[test]
    fn taking_the_retreat_tells_the_watcher_to_look_again() {
        // Otherwise the watcher sleeps out its poll while the session is on a
        // link it did not choose and has not re-armed for.
        let switch = LinkSwitch::new(Route::unbound(at("127.0.0.1:4010"), LinkKind::Wired));
        let route = Route::unbound(at("127.0.0.1:4010"), LinkKind::Wireless);
        switch.arm(Link::bind(route).expect("an ephemeral loopback socket"));
        assert!(switch.armed());

        assert!(switch.take_retreat().is_some());
        assert!(!switch.armed());
        assert!(switch.took_retreat());
        assert!(
            !switch.took_retreat(),
            "one retreat must not be reacted to twice"
        );
    }

    #[test]
    fn disarming_leaves_nothing_for_the_send_loop_to_take() {
        // A standby bound for a route that has since gone is worse than none:
        // the send loop would retreat onto an interface that is not there.
        let switch = LinkSwitch::new(Route::unbound(at("127.0.0.1:4010"), LinkKind::Wired));
        let route = Route::unbound(at("127.0.0.1:4010"), LinkKind::Wireless);
        switch.arm(Link::bind(route).expect("an ephemeral loopback socket"));

        switch.disarm();

        assert!(!switch.armed());
        assert!(switch.take_retreat().is_none());
    }

    #[test]
    fn a_bound_link_is_non_blocking_before_the_send_loop_ever_sees_it() {
        // The send loop reads this socket for the receiver's reports. A
        // blocking read there waits forever the moment there is nothing to
        // read, which from the far end looks like a sender that has crashed.
        let link = Link::bind(Route::unbound(at("127.0.0.1:4010"), LinkKind::Wireless))
            .expect("an ephemeral loopback socket");
        let mut buffer = [0_u8; 8];
        let error = link
            .socket
            .recv_from(&mut buffer)
            .expect_err("nothing has been sent to it");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[cfg(windows)]
    #[test]
    fn the_machine_agrees_with_itself_about_the_link_to_its_own_gateway() {
        // Runs against the real adapter list. It asserts consistency rather
        // than an outcome, because the outcome depends on what is plugged in:
        // whatever observe() says, the label and the header flag must be the
        // same fact rendered twice.
        for adapter in adapters::enumerate().unwrap_or_default() {
            let target = adapter.target(4010);
            let kind = observe(target);
            println!("{} -> {} via {:?}", adapter.description, kind.label(), kind);
            assert_eq!(kind.is_wired(), kind.label() == "usb");
        }
    }
}
