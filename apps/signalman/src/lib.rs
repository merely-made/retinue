#![forbid(unsafe_code)]

//! Signalman: the radio-management application of the retinue family.
//!
//! The signalman sits in the box, sets the routes, works the block sections, and hands out
//! the single-line token — the object exactly one train may hold to enter a single-track
//! section. That is this hardware's channel model by another name: one radio, one mesh at a
//! time, admission by an explicit act.
//!
//! The application is the signal box for a household's radios: which board runs which
//! channel, who is heard, what is delivered, and what the boards themselves report. The
//! radio work itself lives in [`postilion`]; what is here is the operator's side of the
//! glass.
//!
//! This library half currently holds the terminal face's vocabulary, so a future graphical
//! face can share it. The binary is [`main`](../src/main.rs).

use std::time::Duration;

use postilion::{Event, Peer, Sent};

pub mod firmware;

pub use firmware::{
    DeviceCandidate, FirmwareCatalog, FirmwareError, FirmwareInstaller, FirmwareReview,
    FirmwareView, describe_event, event_progress, observe_device, refusal_lines, survey_devices,
    survey_ports,
};

/// How long a send waits for a recipient who has not announced yet.
///
/// Generous on purpose: an address is usually something you were told or met last week, and
/// the owner may simply be between announces. Failing instantly would report "no such
/// person" for somebody standing beside you.
pub const SEND_PATIENCE: Duration = Duration::from_secs(75);

/// What the operator typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// Nothing worth acting on.
    Blank,
    /// List peers heard.
    Peers,
    /// Address somebody by the leading hex of their address.
    To(&'a str),
    /// Send this text to the current recipient.
    Say(&'a str),
    /// Stop.
    Quit,
    /// Announce now, out of cadence.
    Announce,
    /// Show what this station is.
    Who,
    /// An unrecognised slash command.
    Unknown(&'a str),
}

/// Read one line of operator input.
///
/// Slash-prefixed words are commands; everything else is a message. The distinction is by
/// prefix rather than by mode, so a person never has to remember which state they are in.
pub fn parse(line: &str) -> Command<'_> {
    let line = line.trim();
    if line.is_empty() {
        return Command::Blank;
    }
    if let Some(rest) = line.strip_prefix('/') {
        let (word, argument) = match rest.split_once(char::is_whitespace) {
            Some((word, argument)) => (word, argument.trim()),
            None => (rest, ""),
        };
        return match word {
            "peers" => Command::Peers,
            "to" => Command::To(argument),
            "quit" | "exit" => Command::Quit,
            "announce" => Command::Announce,
            "who" | "me" => Command::Who,
            other => Command::Unknown(other),
        };
    }
    Command::Say(line)
}

/// One line describing a peer, for the `/peers` listing.
pub fn describe(peer: &Peer) -> String {
    let name = peer.name.as_deref().unwrap_or("(unnamed)");
    match peer.stamp_cost {
        Some(cost) => format!("  {} {name} stamp={cost}", peer.destination),
        None => format!("  {} {name}", peer.destination),
    }
}

/// One line describing an event, or `None` for events the terminal face does not show.
pub fn render(event: &Event) -> Option<String> {
    Some(match event {
        Event::PeerAppeared(peer) => {
            let name = peer.name.as_deref().unwrap_or("(unnamed)");
            format!("[peer] {} {name} appeared", peer.destination)
        }
        Event::Message { from, title, body } => {
            let title = String::from_utf8_lossy(title);
            let body = String::from_utf8_lossy(body);
            format!("[{from}] {title}: {body}")
        }
        // Shown rather than swallowed: the commonest cause is a sender this station has never
        // heard announce, and a silent drop is indistinguishable from a dead radio.
        Event::Dropped(reason) => format!("[dropped] {reason}"),
    })
}

/// One line describing what became of a send.
pub fn report(sent: &Sent, prefix: &str) -> String {
    match sent {
        Sent::Delivered { mode } => format!("(sent via {mode:?})"),
        Sent::NoSuchPeer => format!("(nobody matching {prefix} announced in time)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_told_from_messages_by_their_slash() {
        assert_eq!(parse(""), Command::Blank);
        assert_eq!(parse("   "), Command::Blank);
        assert_eq!(parse("/peers"), Command::Peers);
        assert_eq!(parse("/to abc123"), Command::To("abc123"));
        assert_eq!(parse("/to   abc123  "), Command::To("abc123"));
        assert_eq!(parse("/quit"), Command::Quit);
        assert_eq!(parse("/exit"), Command::Quit);
        assert_eq!(parse("/announce"), Command::Announce);
        assert_eq!(parse("/who"), Command::Who);
        assert_eq!(parse("/frobnicate"), Command::Unknown("frobnicate"));
        assert_eq!(parse("hello there"), Command::Say("hello there"));
    }

    /// A message that merely mentions a slash is still a message. Only a leading one commands,
    /// or a person could not send "and/or" without being told it is unknown.
    #[test]
    fn only_a_leading_slash_commands() {
        assert_eq!(parse("and/or"), Command::Say("and/or"));
        assert_eq!(parse("  hi /peers"), Command::Say("hi /peers"));
    }

    #[test]
    fn a_bare_to_is_an_empty_prefix_rather_than_a_parse_failure() {
        // Answered by the face as "who?", not crashed on.
        assert_eq!(parse("/to"), Command::To(""));
    }
}
