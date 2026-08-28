//! When to move a running session from one link to another.
//!
//! # The shape of it
//!
//! Two rules, and they are deliberately not symmetric:
//!
//! - **Moving off a link that still works needs proof.** The candidate has to
//!   be there, and answer as the right device, on several consecutive polls
//!   before the session goes anywhere. Upgrading is a nicety: the audio is
//!   already playing, and being wrong costs a migration nobody asked for.
//! - **Moving off a link that has stopped working is immediate.** There is
//!   nothing to weigh. Staying is silence, and any route that verifies beats
//!   a route that is gone.
//!
//! That asymmetry is the hysteresis. A cable that appears and disappears
//! cannot produce a migration every few seconds, because the appearing half
//! costs six seconds of continuous evidence while the disappearing half is
//! free -- so a flapping link spends all its time failing to accumulate a
//! streak, and the session sits still.
//!
//! # And when six seconds is not enough
//!
//! A cable with a bad contact can hold for six seconds, get adopted, and drop
//! again, over and over. Each of those is one migration, which is one glitch,
//! which is exactly what the user complained about. So a wired link that is
//! adopted and lost before it has proved itself doubles the evidence the next
//! one needs, up to a minute. A link that carries audio for half a minute is
//! believed, and the requirement drops back to where it started.
//!
//! The result is that a genuinely bad cable is tried, tried again more slowly,
//! and then left alone -- while a good cable plugged in normally is adopted
//! six seconds later, every time.
//!
//! # Pure on purpose
//!
//! No clock, no sockets, no adapter list. Time is a count of polls, supplied
//! by the caller, so the whole policy can be driven through a synthetic
//! sequence of link states in a test on a machine with nothing plugged into
//! it. What it takes to *make* an [`Observation`] needs a machine; what to do
//! with one does not.

use super::link::{LinkKind, Route};

/// How long the watcher waits between observations.
///
/// Two seconds. The observation costs one walk of the adapter list, which at
/// this rate is invisible, and it bounds how late the session can be to notice
/// a cable at two seconds, which is well inside the time it takes a person to
/// finish plugging one in.
pub const POLL_SECONDS: u64 = 2;

/// Consecutive verified observations before a working link is left for a
/// better one.
///
/// Three, so six seconds. Android's tether bring-up -- USB enumeration, the
/// RNDIS or NCM driver attaching, DHCP, then the receiver binding its own
/// socket -- takes one to three seconds, and the adapter appears part way
/// through it with no gateway and nothing answering. Three polls clears that
/// transient without making the user wait long enough to reach for the mouse.
pub const CONFIRMATIONS: u32 = 3;

/// The most evidence an upgrade may ever be made to wait for.
///
/// Thirty polls, a minute. Past this a link is being refused rather than
/// deferred, and refusing forever would mean a cable that once flapped could
/// never be used again in that session.
pub const MAX_CONFIRMATIONS: u32 = 30;

/// Polls a link must carry audio for before it counts as proved.
///
/// Fifteen, thirty seconds. Long enough that a bad contact does not qualify,
/// short enough that a user who plugs in, gets a migration, and then
/// deliberately unplugs is not punished for it a minute later.
pub const STABLE_POLLS: u32 = 15;

/// What the user asked for, from the transport preference in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preference {
    /// Take the best link available. The default, and the one this feature
    /// exists for.
    #[default]
    Auto,
    /// Stay on the wireless link. A cable appearing is ignored.
    ///
    /// Falling back is still allowed: the alternative to a dead link is not
    /// another link, it is silence.
    Wireless,
    /// Prefer the cable. Behaves as [`Self::Auto`] does, because there is no
    /// coherent reading of "USB only" that does not mean "stop playing when
    /// the cable comes out".
    Wired,
}

impl Preference {
    /// Read the string the settings page stores.
    ///
    /// Anything unrecognised is [`Self::Auto`]. A preference that cannot be
    /// parsed is not a reason to strand the user on one link.
    #[must_use]
    pub fn parse(text: Option<&str>) -> Self {
        match text {
            Some("wifi") => Self::Wireless,
            Some("usb") => Self::Wired,
            _ => Self::Auto,
        }
    }

    /// Whether a working wireless link may be left for a cable.
    ///
    /// Public because the watcher reads it too: when no upgrade is allowed
    /// there is no reason to spend a probe proving a candidate the policy has
    /// already decided not to take. It still probes when the link in use has
    /// died, because then the candidate is not an upgrade but the only option.
    #[must_use]
    pub const fn allows_upgrade(self) -> bool {
        !matches!(self, Self::Wireless)
    }
}

/// What one poll saw.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The link the session is on right now, as the send loop reports it.
    ///
    /// Read rather than remembered, because the send loop can move the session
    /// without asking: it sees a route stop accepting datagrams tens of
    /// milliseconds before an adapter walk would.
    pub current: LinkKind,
    /// Whether the route the session is on still exists.
    pub current_alive: bool,
    /// A wired route to the same device, proved this poll. `None` covers both
    /// "no cable" and "a cable with something on it that is not our phone",
    /// which are the same thing as far as this decision goes.
    pub wired: Option<Route>,
    /// The most recently proved route that is not a cable, for retreating to.
    pub wireless: Option<Route>,
}

/// Why a migration is being made, which is what the user is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A better link became available and proved itself.
    Upgrade,
    /// The link in use has gone.
    Fallback,
}

/// What the policy decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to do. The overwhelmingly common answer.
    Stay,
    /// Move the session to this route.
    Move(Route, Reason),
}

/// The migration policy, and the memory that makes it hysteretic.
#[derive(Debug)]
pub struct Policy {
    preference: Preference,
    /// Consecutive polls a wired candidate has been verified for.
    streak: u32,
    /// How long a streak the next upgrade needs. Grows when a wired link is
    /// adopted and lost early.
    required: u32,
    /// Polls the session has spent on the link it is on.
    on_current: u32,
    /// The link the last observation reported, so a change made by the send
    /// loop is noticed here as well as one made by this policy.
    seen: Option<LinkKind>,
}

impl Policy {
    /// A policy for a session that has not migrated yet.
    #[must_use]
    pub const fn new(preference: Preference) -> Self {
        Self {
            preference,
            streak: 0,
            required: CONFIRMATIONS,
            on_current: 0,
            seen: None,
        }
    }

    /// How much evidence the next upgrade needs, for the log and for tests.
    #[must_use]
    pub const fn required(&self) -> u32 {
        self.required
    }

    /// Fold in one poll and say what to do about it.
    pub fn observe(&mut self, seen: &Observation) -> Decision {
        self.age(seen.current);

        match seen.current {
            // A group address has no far end to follow. There is no link to
            // improve on and nothing that can be proved to be the same device,
            // so the session stays where the user pointed it.
            LinkKind::Multicast => Decision::Stay,
            LinkKind::Wired => self.on_wired(seen),
            LinkKind::Wireless => self.on_wireless(seen),
        }
    }

    /// Account for the link the session is on, whoever put it there.
    fn age(&mut self, current: LinkKind) {
        if self.seen == Some(current) {
            self.on_current = self.on_current.saturating_add(1);
            // A link that has carried audio this long is a link, not a
            // coincidence. Anything it did before is forgiven.
            if current == LinkKind::Wired && self.on_current >= STABLE_POLLS {
                self.required = CONFIRMATIONS;
            }
            return;
        }

        // The link changed. If a wired one went away before it had proved
        // itself, the next one has to try harder -- and this is where a
        // retreat the send loop made on its own is charged, exactly as one
        // this policy ordered would be.
        if self.seen == Some(LinkKind::Wired)
            && current != LinkKind::Wired
            && self.on_current < STABLE_POLLS
        {
            self.required = (self.required.saturating_mul(2)).min(MAX_CONFIRMATIONS);
        }

        self.seen = Some(current);
        // One, not zero: this poll is a poll spent on the new link, and
        // counting from zero would make every window one observation longer
        // than the constant that names it.
        self.on_current = 1;
        self.streak = 0;
    }

    /// The session is on a cable.
    fn on_wired(&mut self, seen: &Observation) -> Decision {
        // Nothing better exists than the link already in use, so the streak
        // has no meaning here and must not carry over into the next spell on
        // Wi-Fi as credit already earned.
        self.streak = 0;

        if seen.current_alive {
            return Decision::Stay;
        }

        match &seen.wireless {
            Some(route) => Decision::Move(route.clone(), Reason::Fallback),
            // The cable is out and nothing else has answered. Staying is not a
            // choice so much as the absence of one, and it keeps the session
            // alive for the moment Wi-Fi comes back.
            None => Decision::Stay,
        }
    }

    /// The session is on Wi-Fi, or on anything else that is not a cable.
    fn on_wireless(&mut self, seen: &Observation) -> Decision {
        if !seen.current_alive {
            // The radio has gone. A verified cable is not an upgrade now, it
            // is the only thing there is, and waiting six seconds for it would
            // be six seconds of nothing.
            self.streak = 0;
            return match &seen.wired {
                Some(route) => Decision::Move(route.clone(), Reason::Fallback),
                None => Decision::Stay,
            };
        }

        if !self.preference.allows_upgrade() {
            self.streak = 0;
            return Decision::Stay;
        }

        let Some(route) = &seen.wired else {
            // Evidence has to be consecutive. A candidate that comes and goes
            // never accumulates, which is the whole of the anti-thrash rule.
            self.streak = 0;
            return Decision::Stay;
        };

        self.streak = self.streak.saturating_add(1);
        if self.streak < self.required {
            return Decision::Stay;
        }

        self.streak = 0;
        Decision::Move(route.clone(), Reason::Upgrade)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn at(address: &str) -> SocketAddr {
        address.parse().expect("a literal address")
    }

    fn cable() -> Route {
        Route {
            target: at("10.114.89.244:4010"),
            bind: at("10.114.89.252:0"),
            kind: LinkKind::Wired,
        }
    }

    fn radio() -> Route {
        Route::unbound(at("192.168.1.42:4010"), LinkKind::Wireless)
    }

    /// A poll on Wi-Fi that is working, with `wired` as the candidate.
    fn on_wifi(wired: Option<Route>) -> Observation {
        Observation {
            current: LinkKind::Wireless,
            current_alive: true,
            wired,
            wireless: Some(radio()),
        }
    }

    /// A poll on the cable, `alive` saying whether it is still there.
    fn on_usb(alive: bool) -> Observation {
        Observation {
            current: LinkKind::Wired,
            current_alive: alive,
            wired: alive.then(cable),
            wireless: Some(radio()),
        }
    }

    /// Drive the same observation through `times` polls.
    fn run(policy: &mut Policy, poll: &Observation, times: u32) -> Vec<Decision> {
        (0..times).map(|_| policy.observe(poll)).collect()
    }

    #[test]
    fn a_cable_that_stays_plugged_in_is_adopted_after_the_confirmation_window() {
        // The feature. The user plugs in and does nothing else.
        let mut policy = Policy::new(Preference::Auto);
        let decisions = run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);

        assert_eq!(decisions[0], Decision::Stay);
        assert_eq!(decisions[1], Decision::Stay);
        assert_eq!(
            decisions[2],
            Decision::Move(cable(), Reason::Upgrade),
            "six seconds of a verified cable and the session did not move"
        );
    }

    #[test]
    fn a_cable_that_is_not_there_yet_never_accumulates_a_streak() {
        // Android's tether bring-up: the adapter appears before anything
        // answers on it, so the candidate flickers in and out.
        let mut policy = Policy::new(Preference::Auto);
        let flicker = [
            on_wifi(Some(cable())),
            on_wifi(None),
            on_wifi(Some(cable())),
            on_wifi(None),
            on_wifi(Some(cable())),
            on_wifi(None),
        ];

        for poll in &flicker {
            assert_eq!(
                policy.observe(poll),
                Decision::Stay,
                "a half-up adapter was adopted"
            );
        }
    }

    #[test]
    fn an_unverified_cable_is_never_adopted_however_long_it_sits_there() {
        // `wired: None` is what an unverified candidate looks like from here:
        // a phone that is not ours, or one that did not answer. Time must not
        // be able to turn that into a yes.
        let mut policy = Policy::new(Preference::Auto);
        for decision in run(&mut policy, &on_wifi(None), 40) {
            assert_eq!(decision, Decision::Stay);
        }
    }

    #[test]
    fn the_cable_coming_out_falls_back_at_once_rather_than_after_a_window() {
        // Asymmetry. Waiting for confirmation here would be six seconds of
        // silence to avoid a migration that is already unavoidable.
        let mut policy = Policy::new(Preference::Auto);
        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);

        let decision = policy.observe(&on_usb(false));
        assert_eq!(decision, Decision::Move(radio(), Reason::Fallback));
    }

    #[test]
    fn a_cable_that_keeps_dropping_is_tried_less_and_less_often() {
        // The complaint, in its worst form: a bad contact. Each cycle must
        // cost more evidence than the last, or the session migrates every few
        // seconds forever.
        let mut policy = Policy::new(Preference::Auto);
        assert_eq!(policy.required(), CONFIRMATIONS);

        let mut adoptions = 0;
        let mut required = Vec::new();
        for _ in 0..4 {
            // However long it takes, adopt it.
            for _ in 0..MAX_CONFIRMATIONS {
                if matches!(
                    policy.observe(&on_wifi(Some(cable()))),
                    Decision::Move(_, Reason::Upgrade)
                ) {
                    adoptions += 1;
                    break;
                }
            }
            // It holds for a moment and then drops.
            policy.observe(&on_usb(true));
            policy.observe(&on_usb(false));
            // And the fallback lands.
            policy.observe(&on_wifi(None));
            required.push(policy.required());
        }

        assert_eq!(adoptions, 4, "the cable stopped being tried altogether");
        assert_eq!(
            required,
            vec![6, 12, 24, 30],
            "the requirement did not back off, or did not stop backing off"
        );
    }

    #[test]
    fn a_cable_that_holds_for_half_a_minute_is_forgiven_its_history() {
        // Otherwise one bad session poisons the rest of it, and a user who
        // unplugs deliberately is punished for it a minute later.
        let mut policy = Policy::new(Preference::Auto);
        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);
        policy.observe(&on_usb(true));
        policy.observe(&on_usb(false));
        policy.observe(&on_wifi(None));
        assert_eq!(policy.required(), CONFIRMATIONS * 2);

        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS * 2);
        run(&mut policy, &on_usb(true), STABLE_POLLS);

        assert_eq!(
            policy.required(),
            CONFIRMATIONS,
            "a link that carried audio for thirty seconds is still on probation"
        );
    }

    #[test]
    fn a_retreat_the_send_loop_made_on_its_own_is_charged_the_same_way() {
        // The send loop can move the session between polls, on send failures
        // this policy never sees. If that path did not back off, the cheap
        // route around the hysteresis would be the one that runs.
        let mut policy = Policy::new(Preference::Auto);
        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);
        policy.observe(&on_usb(true));

        // No `on_usb(false)` at all: the next thing the policy sees is a
        // session already back on Wi-Fi.
        policy.observe(&on_wifi(None));

        assert_eq!(policy.required(), CONFIRMATIONS * 2);
    }

    #[test]
    fn a_working_cable_is_left_alone_indefinitely() {
        let mut policy = Policy::new(Preference::Auto);
        for decision in run(&mut policy, &on_usb(true), 100) {
            assert_eq!(decision, Decision::Stay);
        }
    }

    #[test]
    fn the_cable_coming_out_with_nowhere_to_go_keeps_the_session_alive() {
        // Wi-Fi is off as well. Stopping would be a decision the user did not
        // ask for; staying costs nothing and recovers by itself.
        let mut policy = Policy::new(Preference::Auto);
        let decision = policy.observe(&Observation {
            current: LinkKind::Wired,
            current_alive: false,
            wired: None,
            wireless: None,
        });
        assert_eq!(decision, Decision::Stay);
    }

    #[test]
    fn the_radio_going_away_takes_a_verified_cable_immediately() {
        // No confirmation window: there is nothing to weigh a candidate
        // against when the alternative is silence.
        let mut policy = Policy::new(Preference::Auto);
        let decision = policy.observe(&Observation {
            current: LinkKind::Wireless,
            current_alive: false,
            wired: Some(cable()),
            wireless: None,
        });
        assert_eq!(decision, Decision::Move(cable(), Reason::Fallback));
    }

    #[test]
    fn a_user_who_asked_to_stay_on_wifi_is_not_moved_onto_a_cable() {
        let mut policy = Policy::new(Preference::Wireless);
        for decision in run(&mut policy, &on_wifi(Some(cable())), 40) {
            assert_eq!(decision, Decision::Stay);
        }
    }

    #[test]
    fn a_user_who_asked_to_stay_on_wifi_is_still_rescued_from_a_dead_radio() {
        // The preference is about which link is nicer, not about whether the
        // audio should stop.
        let mut policy = Policy::new(Preference::Wireless);
        let decision = policy.observe(&Observation {
            current: LinkKind::Wireless,
            current_alive: false,
            wired: Some(cable()),
            wireless: None,
        });
        assert_eq!(decision, Decision::Move(cable(), Reason::Fallback));
    }

    #[test]
    fn a_multicast_session_is_never_migrated() {
        // Nobody in particular is on the other end, so nothing can be proved
        // to be the same device and there is no better link to move to.
        let mut policy = Policy::new(Preference::Auto);
        let decision = policy.observe(&Observation {
            current: LinkKind::Multicast,
            current_alive: true,
            wired: Some(cable()),
            wireless: Some(radio()),
        });
        assert_eq!(decision, Decision::Stay);
    }

    #[test]
    fn preferences_are_read_from_what_the_settings_page_stores() {
        assert_eq!(Preference::parse(Some("auto")), Preference::Auto);
        assert_eq!(Preference::parse(Some("wifi")), Preference::Wireless);
        assert_eq!(Preference::parse(Some("usb")), Preference::Wired);
        // A stored value from a future build, or none at all. Neither is a
        // reason to strand the session on one link.
        assert_eq!(Preference::parse(Some("something else")), Preference::Auto);
        assert_eq!(Preference::parse(None), Preference::Auto);
    }

    #[test]
    fn preferring_the_cable_still_falls_back_off_one_that_has_gone() {
        let mut policy = Policy::new(Preference::Wired);
        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);
        assert_eq!(
            policy.observe(&on_usb(false)),
            Decision::Move(radio(), Reason::Fallback)
        );
    }

    #[test]
    fn one_streak_is_not_spent_twice() {
        // After a migration the count starts again, or the session would
        // bounce straight back the next time a candidate appeared.
        let mut policy = Policy::new(Preference::Auto);
        run(&mut policy, &on_wifi(Some(cable())), CONFIRMATIONS);
        run(&mut policy, &on_usb(true), 2);
        policy.observe(&on_usb(false));

        // Back on Wi-Fi with the cable visible again: it must serve the full
        // (now doubled) window rather than moving on the first poll.
        let decisions = run(&mut policy, &on_wifi(Some(cable())), 5);
        assert!(
            decisions.iter().all(|decision| *decision == Decision::Stay),
            "a stale streak carried a migration through early"
        );
    }
}
