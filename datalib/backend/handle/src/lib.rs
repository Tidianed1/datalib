//! A handle: one identifier for a person in one namespace, normalized so
//! that the same person reached two ways reads the same.
//!
//! A handle is `<kind>:<value>` — `email:riker@enterprise.org`,
//! `tel:+12025550123`, `slack:T01/U02`. Where a native id *is* an email
//! address or a phone number it becomes one of those, not a per-app kind,
//! so one link covers every app that reaches a person by that number.
//! Renders write handles into the markdown (`data-handle`) and the
//! contacts app links them to contacts; nothing here knows what a contact
//! is. `docs/dev/plans/contacts.md` has the design.

use std::fmt;

use serde::{Deserialize, Serialize};
use strum::{EnumString, IntoStaticStr, VariantArray};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, IntoStaticStr, VariantArray)]
#[strum(serialize_all = "snake_case")]
pub enum HandleKind {
    Email,
    Tel,
    Slack,
}

impl HandleKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// `None` for a spelling this build does not know.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Handle(String);

impl From<Handle> for String {
    fn from(h: Handle) -> String {
        h.0
    }
}

impl TryFrom<String> for Handle {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        Handle::parse(&s).ok_or_else(|| format!("{s:?} is not a handle"))
    }
}

impl Handle {
    /// An email address, lowercased. `Name <addr>` is not accepted:
    /// separating the name is the caller's parse, not a guess made here.
    pub fn email(addr: &str) -> Option<Self> {
        let addr = addr.trim();
        let (local, domain) = addr.split_once('@')?;
        let well_formed = !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && domain.contains('.')
            && !addr
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | ','));
        well_formed.then(|| Self::of(HandleKind::Email, &addr.to_lowercase()))
    }

    /// A phone number already in international form: a leading `+`, then
    /// the country code and number. Spaces, dashes, dots and parentheses
    /// are dropped. A number without its country code is refused rather
    /// than given one by guess.
    pub fn tel(number: &str) -> Option<Self> {
        let rest = number.trim().strip_prefix('+')?;
        let mut digits = String::with_capacity(rest.len());
        for c in rest.chars() {
            match c {
                '0'..='9' => digits.push(c),
                ' ' | '-' | '.' | '(' | ')' => {}
                _ => return None,
            }
        }
        // E.164 allows at most 15 digits; fewer than 7 is a short code.
        // Under country code 1 (NANP) every number has ten digits after
        // the 1, so anything shorter is a short code someone put `+1` on.
        let nanp = digits.starts_with('1');
        let plausible = if nanp {
            digits.len() == 11
        } else {
            (7..=15).contains(&digits.len()) && !digits.starts_with('0')
        };
        plausible.then(|| Self::of(HandleKind::Tel, &format!("+{digits}")))
    }

    /// A WhatsApp JID. A person's JID is their phone number at
    /// `s.whatsapp.net`, so it becomes a `tel:` handle; a group
    /// (`@g.us`), a linked-device id (`@lid`) or anything else is not a
    /// phone number and has no handle yet.
    pub fn whatsapp_jid(jid: &str) -> Option<Self> {
        let number = jid.strip_suffix("@s.whatsapp.net")?;
        let number = number.split_once(':').map_or(number, |(n, _device)| n);
        Self::tel(&format!("+{number}"))
    }

    /// A Slack user, which is only unique within its workspace.
    pub fn slack(team_id: &str, user_id: &str) -> Option<Self> {
        let ok =
            |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        (ok(team_id) && ok(user_id))
            .then(|| Self::of(HandleKind::Slack, &format!("{team_id}/{user_id}")))
    }

    /// A handle as written by [`Handle::as_str`]. `None` for an unknown
    /// kind or a value its kind would not have produced.
    pub fn parse(s: &str) -> Option<Self> {
        let (kind, value) = s.split_once(':')?;
        let handle = match HandleKind::parse(kind)? {
            HandleKind::Email => Self::email(value),
            HandleKind::Tel => Self::tel(value),
            HandleKind::Slack => {
                let (team, user) = value.split_once('/')?;
                Self::slack(team, user)
            }
        }?;
        (handle.0 == s).then_some(handle)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> HandleKind {
        let kind = self.0.split_once(':').map_or("", |(k, _)| k);
        HandleKind::parse(kind).expect("a Handle is only built with a known kind")
    }

    pub fn value(&self) -> &str {
        self.0.split_once(':').map_or("", |(_, v)| v)
    }

    fn of(kind: HandleKind, value: &str) -> Self {
        Self(format!("{}:{value}", kind.as_str()))
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_lowercased_and_trimmed() {
        let h = Handle::email("  Riker@Enterprise.ORG ").unwrap();
        assert_eq!(h.as_str(), "email:riker@enterprise.org");
        assert_eq!(h.kind(), HandleKind::Email);
        assert_eq!(h.value(), "riker@enterprise.org");
    }

    #[test]
    fn email_refuses_what_is_not_one_address() {
        for bad in [
            "",
            "riker",
            "@enterprise.org",
            "riker@",
            "riker@localhost",
            "Will Riker <riker@enterprise.org>",
            "riker@enterprise.org, troi@enterprise.org",
            "a@b@c.org",
        ] {
            assert_eq!(Handle::email(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tel_keeps_digits_and_requires_a_country_code() {
        assert_eq!(
            Handle::tel("+1 (555) 012-3456").unwrap().as_str(),
            "tel:+15550123456"
        );
        assert_eq!(
            Handle::tel("+44 20.7946.0958").unwrap().as_str(),
            "tel:+442079460958"
        );
        for bad in [
            "5550123456",
            "+12345",
            "+1555012345678901",
            "+1 555 CALL NOW",
            "+0555012345",
            "+1123456",
            "+1 555 012 345",
            "+1 202 555 01234",
        ] {
            assert_eq!(Handle::tel(bad), None, "{bad:?}");
        }
    }

    /// A WhatsApp sender and the same person's Signal number must be one
    /// handle, or a link made in one app does nothing in the other.
    #[test]
    fn whatsapp_person_jid_is_the_same_tel_handle_as_the_number() {
        let wa = Handle::whatsapp_jid("15550123456@s.whatsapp.net").unwrap();
        assert_eq!(wa, Handle::tel("+1 555 012 3456").unwrap());
        assert_eq!(
            Handle::whatsapp_jid("15550123456:12@s.whatsapp.net"),
            Some(wa)
        );
        assert_eq!(Handle::whatsapp_jid("120363000000000000@g.us"), None);
        assert_eq!(Handle::whatsapp_jid("123456789012345@lid"), None);
    }

    #[test]
    fn slack_is_scoped_to_its_workspace() {
        assert_eq!(
            Handle::slack("T01", "U02").unwrap().as_str(),
            "slack:T01/U02"
        );
        assert_eq!(Handle::slack("", "U02"), None);
        assert_eq!(Handle::slack("T01", "U02/x"), None);
        assert_eq!(
            Handle::slack("T_NCC1701D", "U_PICARD").unwrap().as_str(),
            "slack:T_NCC1701D/U_PICARD"
        );
    }

    #[test]
    fn parse_round_trips_and_refuses_unnormalized_spellings() {
        for h in [
            Handle::email("riker@enterprise.org").unwrap(),
            Handle::tel("+15550123456").unwrap(),
            Handle::slack("T01", "U02").unwrap(),
        ] {
            assert_eq!(Handle::parse(h.as_str()), Some(h.clone()));
        }
        assert_eq!(Handle::parse("email:Riker@Enterprise.org"), None);
        assert_eq!(Handle::parse("tel:+1 555 012 3456"), None);
        assert_eq!(Handle::parse("fax:+15550123456"), None);
        assert_eq!(Handle::parse("riker@enterprise.org"), None);
    }

    #[test]
    fn every_kind_spells_and_parses_back() {
        for kind in HandleKind::VARIANTS {
            assert_eq!(HandleKind::parse(kind.as_str()), Some(*kind));
        }
    }

    /// A handle read back from a store goes through `parse`, so a row
    /// cannot carry a spelling the normalizers would not have produced.
    #[test]
    fn serde_round_trips_through_parse() {
        let h = Handle::tel("+15550123456").unwrap();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"tel:+15550123456\"");
        assert_eq!(serde_json::from_str::<Handle>(&json).unwrap(), h);
        assert!(serde_json::from_str::<Handle>("\"tel:+1 555\"").is_err());
    }
}
