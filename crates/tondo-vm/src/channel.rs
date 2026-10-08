//! Sealed channel endpoint identities shared by compiler and bytecode checks.
//!
//! The bootstrap Unit placeholder describes neither endpoint ownership nor
//! transfer capabilities. These identities keep those rules independent of
//! nominal representation; they do not publish a native channel ABI.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelEndpointKind {
    Sender,
    Receiver,
}

impl ChannelEndpointKind {
    pub fn from_identity(identity: &str, generic_arity: u32) -> Option<Self> {
        if generic_arity != 1 {
            return None;
        }
        match identity {
            "@27:toolchain:std:0.1-bootstrap::channel::type::Sender" => Some(Self::Sender),
            "@27:toolchain:std:0.1-bootstrap::channel::type::Receiver" => Some(Self::Receiver),
            _ => None,
        }
    }

    pub const fn is_discardable(self) -> bool {
        matches!(self, Self::Sender)
    }

    pub const fn owns_terminal(self) -> bool {
        matches!(self, Self::Receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelEndpointKind;

    #[test]
    fn channel_endpoint_identity_is_exact_and_arity_bound() {
        for (name, expected) in [
            ("Sender", ChannelEndpointKind::Sender),
            ("Receiver", ChannelEndpointKind::Receiver),
        ] {
            let identity = format!("@27:toolchain:std:0.1-bootstrap::channel::type::{name}");
            assert_eq!(
                ChannelEndpointKind::from_identity(&identity, 1),
                Some(expected)
            );
            for arity in [0, 2, u32::MAX] {
                assert_eq!(ChannelEndpointKind::from_identity(&identity, arity), None);
            }
            for wrong in [
                identity.replacen("@27:", "@26:", 1),
                identity.replacen("channel", "user", 1),
                identity.replacen("::type::", "::trait::", 1),
                format!("{identity}::Nested"),
                format!("prefix{identity}"),
            ] {
                assert_eq!(ChannelEndpointKind::from_identity(&wrong, 1), None);
            }
        }
        assert!(ChannelEndpointKind::Sender.is_discardable());
        assert!(!ChannelEndpointKind::Sender.owns_terminal());
        assert!(!ChannelEndpointKind::Receiver.is_discardable());
        assert!(ChannelEndpointKind::Receiver.owns_terminal());
    }
}
