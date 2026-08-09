//! Who is asking, as a thing the runtime **decides** rather than a thing the client asserts.
//!
//! [`docs/42-security-assurance.md`](../../../../../docs/42-security-assurance.md) §42.6's first
//! bullet: "**Claim any identity.** `actor` arrives in the client's own `hello` frame … Every
//! ownership check in every corpus program is therefore enforced against a value the caller
//! chooses." [`docs/43`](../../../../../docs/43-threat-model.md) §43.4 records it as the gap that
//! makes the difference between §3.5's *proven* properties and a program's own rules, and §42.5
//! names it as the most likely misquotation of this project's security story: "a capability
//! required outside the chokepoint has no holder" is true and proven; "only the owner may toggle
//! their todo" was, until this module, enforced against a self-asserted string.
//!
//! # What this is, and what it is not
//!
//! It is a **seam**, in the sense `beck_core::clock` is one: a trait with the current behaviour as
//! one implementation and a verifying implementation as another, so that identity is a thing an
//! operator *chooses* rather than a thing the runtime assumes. Two implementations, because a seam
//! with one is an abstraction nobody has checked.
//!
//! What it does is remove the thing that made the gap structural: an actor arrives through one
//! function that can **refuse**, and nothing else in the runtime can mint one.
//!
//! # The third implementation is in [`crate::oidc`]
//!
//! Both providers here are **symmetric or nothing**: `DevIdentity` verifies nothing, and
//! `SignedIdentity` verifies a secret this process also holds, so neither can tell "the user
//! authenticated" from "this process said so". [`crate::oidc::RelyingParty`] is the asymmetric one
//! — [`10`](../../../../../docs/10-decisions.md) D6's OIDC relying party — and it is a third
//! implementation of this trait rather than a change to it, which is what the seam existed for.

use std::collections::BTreeMap;
use std::sync::Arc;

/// A verified identity. Nothing constructs one except an [`Identity`] implementation.
///
/// The privacy of the field is the point: a `String` from a frame cannot become an `Actor` by
/// being assigned to one, so "where did this actor come from" has exactly one answer everywhere it
/// is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    name: Arc<str>,
    claims: BTreeMap<Arc<str>, Arc<str>>,
}

impl Actor {
    /// The one constructor, and it is `pub(crate)` because every [`Identity`] implementation is in
    /// this crate. A `String` that arrived in a frame becomes an actor by being *verified*, which
    /// is one function call away from being audited rather than spread over the runtime.
    pub(crate) fn verified(name: &str, claims: BTreeMap<Arc<str>, Arc<str>>) -> Actor {
        Actor {
            name: Arc::from(name),
            claims,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The claims this identity carries.
    ///
    /// D6 asks for "claims → `Session` capability mapping", and both halves are here now: these
    /// are the claims [`crate::oidc`] verified, and [`crate::program`] puts them on the `Session`
    /// the program sees. They do **not** reach the log — an envelope carries the actor's name and
    /// nothing else, because a fold that read a claim would be a fold whose replay depended on
    /// what the issuer was saying at the time
    /// ([`docs/95`](../../../../../docs/95-oidc-relying-party-report.md) §95.4).
    pub fn claims(&self) -> &BTreeMap<Arc<str>, Arc<str>> {
        &self.claims
    }
}

/// Whoever a proposal is charged to, on the way into [`crate::App::propose`].
///
/// A wrapper rather than an `impl From<String> for Actor`, and the difference is the whole point:
/// a conversion on `Actor` itself would be a public way to make one out of a string, which is what
/// [`Actor`]'s private field exists to prevent. This converts into a *proposal's* actor — a
/// harness naming one, a benchmark, `beck test` — and the wire path does not use it, because
/// `session.rs` already holds an `Actor` that [`Identity::verify`] produced.
#[derive(Clone, Debug)]
pub struct Proposer(pub(crate) Actor);

impl From<Actor> for Proposer {
    fn from(actor: Actor) -> Proposer {
        Proposer(actor)
    }
}

impl From<String> for Proposer {
    fn from(name: String) -> Proposer {
        Proposer(Actor::verified(&name, BTreeMap::new()))
    }
}

impl From<&str> for Proposer {
    fn from(name: &str) -> Proposer {
        Proposer(Actor::verified(name, BTreeMap::new()))
    }
}

/// Why an identity was refused. One reason per way a connection can be wrong about who it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// No credential at all where one was required.
    Missing,
    /// A credential that does not verify — a wrong signature, or one for a different secret.
    Invalid,
    /// A credential that verifies and has expired.
    Expired,
}

impl Rejected {
    /// What the client is told. Deliberately coarse: a client learns that it was refused and not
    /// which of the three it was, because the difference is useful to an attacker and to nobody
    /// else. The *operator* gets the distinction, in the log.
    pub fn message(&self) -> &'static str {
        "unauthenticated"
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Rejected::Missing => "no credential",
            Rejected::Invalid => "credential does not verify",
            Rejected::Expired => "credential has expired",
        }
    }
}

/// How a claimed identity becomes a verified one.
pub trait Identity: Send + Sync + std::fmt::Debug {
    /// Verify what a client said about itself.
    ///
    /// `claim` is the raw string from the `hello` frame or the `?actor=` query — whatever the
    /// client sent, unmodified, including empty.
    fn verify(&self, claim: &str) -> Result<Actor, Rejected>;

    /// What this provider is, for the dashboard and for the startup line. An operator who cannot
    /// tell from the logs whether authentication is on does not have authentication.
    fn kind(&self) -> &'static str;

    /// Whether this provider verifies anything at all.
    ///
    /// Exists so the runtime can *say* it is unauthenticated rather than leaving it to be
    /// inferred. `docs/42` §42.6's whole point is that an absent control was invisible.
    fn verifies(&self) -> bool {
        true
    }

    /// The browser-facing half, for a provider that can run a login flow.
    ///
    /// `None` for both of this module's providers, and that is the honest answer rather than a
    /// missing feature: neither *issues* anything, so neither has anywhere to send a browser.
    /// [`crate::oidc::RelyingParty`] does, and the HTTP edge asks this rather than being told which
    /// provider is configured.
    fn login(&self) -> Option<&crate::oidc::RelyingParty> {
        None
    }
}

/// Believe whatever the client says. The behaviour every phase before this had.
///
/// It is the right default for `beck run` on a laptop and for the corpus harnesses, and it is
/// wrong for anything reachable by a stranger — which is why it now has a name, a `kind()` that
/// says "dev", and a `verifies()` that says no.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevIdentity;

impl Identity for DevIdentity {
    fn verify(&self, claim: &str) -> Result<Actor, Rejected> {
        // An empty actor is still refused: a program's ownership checks compare against it, and
        // "" matching "" would make every anonymous client the owner of every anonymous record.
        if claim.is_empty() {
            return Err(Rejected::Missing);
        }
        Ok(Actor::verified(claim, BTreeMap::new()))
    }

    fn kind(&self) -> &'static str {
        "dev"
    }

    fn verifies(&self) -> bool {
        false
    }
}

/// A credential signed with a secret this process holds.
///
/// The credential is `<payload>.<mac>`, where `payload` is
/// `actor;expiry_millis;key=value;key=value…` and `mac` is a keyed BLAKE3 of it under the shared
/// secret, hex-encoded. Verification is a constant-time comparison of the recomputed tag.
///
/// **This is a symmetric scheme, and its limits are the point of writing them here.** It suits a
/// gateway that mints credentials for a Beck process behind it — the shape a rung-1 deployment
/// actually has — and it does not suit a public identity provider, because everything that can
/// verify a credential can also mint one. An asymmetric verifier is D6's OIDC work and needs a
/// signature library ([`48`](../../../../../docs/48-identity-report.md) §48.5).
///
/// BLAKE3's keyed mode is a MAC by construction and is already in this workspace's dependency
/// graph, so this costs no new dependency and no hand-rolled cryptography — the two ways a module
/// like this usually goes wrong.
#[derive(Debug)]
pub struct SignedIdentity {
    key: [u8; 32],
    clock: Arc<dyn beck_core::clock::Clock>,
}

impl SignedIdentity {
    /// A verifier for a shared secret of any length, stretched to BLAKE3's key size by its own
    /// derivation function rather than by truncation or padding.
    pub fn new(secret: &str, clock: Arc<dyn beck_core::clock::Clock>) -> SignedIdentity {
        SignedIdentity {
            key: blake3::derive_key("beck identity credential v1", secret.as_bytes()),
            clock,
        }
    }

    /// Mint a credential. Present so a test, a gateway written in Beck, or `beck run --auth` can
    /// produce one — and so the format has exactly one implementation rather than a description.
    pub fn mint(&self, actor: &str, expires_at_millis: i64, claims: &[(&str, &str)]) -> String {
        let payload = Self::payload(actor, expires_at_millis, claims);
        let mac = blake3::keyed_hash(&self.key, payload.as_bytes());
        format!("{payload}.{}", mac.to_hex())
    }

    fn payload(actor: &str, expires_at_millis: i64, claims: &[(&str, &str)]) -> String {
        let mut out = format!("{actor};{expires_at_millis}");
        for (k, v) in claims {
            out.push(';');
            out.push_str(k);
            out.push('=');
            out.push_str(v);
        }
        out
    }
}

impl Identity for SignedIdentity {
    fn verify(&self, claim: &str) -> Result<Actor, Rejected> {
        if claim.is_empty() {
            return Err(Rejected::Missing);
        }
        let (payload, mac) = claim.rsplit_once('.').ok_or(Rejected::Invalid)?;
        let expected = blake3::keyed_hash(&self.key, payload.as_bytes());
        // `Hash`'s `PartialEq` is constant-time, which is why the comparison is written this way
        // round rather than against the hex string.
        let given: blake3::Hash = mac.parse().map_err(|_| Rejected::Invalid)?;
        if expected != given {
            return Err(Rejected::Invalid);
        }

        let mut parts = payload.split(';');
        let name = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or(Rejected::Invalid)?;
        let expiry: i64 = parts
            .next()
            .ok_or(Rejected::Invalid)?
            .parse()
            .map_err(|_| Rejected::Invalid)?;
        // The clock is the injected one, so a test states the instant and a replay is not at the
        // mercy of when it ran (`beck_core::clock`, and F11's constraint).
        if self.clock.now_millis() >= expiry {
            return Err(Rejected::Expired);
        }
        let claims = parts
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (Arc::from(k), Arc::from(v)))
            .collect();
        Ok(Actor::verified(name, claims))
    }

    fn kind(&self) -> &'static str {
        "signed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::clock::ManualClock;

    fn at(ms: i64) -> Arc<ManualClock> {
        Arc::new(ManualClock::at(ms))
    }

    #[test]
    fn dev_identity_believes_the_client_and_refuses_an_empty_name() {
        let id = DevIdentity;
        assert_eq!(id.verify("alice").expect("believed").name(), "alice");
        assert_eq!(id.verify(""), Err(Rejected::Missing));
        assert!(!id.verifies(), "and it says it is not verifying anything");
    }

    #[test]
    fn a_signed_credential_round_trips_with_its_claims() {
        let clock = at(1_000);
        let id = SignedIdentity::new("a shared secret", clock.clone());
        let token = id.mint("alice", 2_000, &[("role", "admin"), ("tenant", "acme")]);
        let actor = id.verify(&token).expect("it verifies");
        assert_eq!(actor.name(), "alice");
        assert_eq!(
            actor.claims().get("role").map(|s| s.as_ref()),
            Some("admin")
        );
        assert_eq!(
            actor.claims().get("tenant").map(|s| s.as_ref()),
            Some("acme")
        );
    }

    /// The whole point: a client cannot name itself.
    #[test]
    fn a_name_without_a_signature_is_refused() {
        let id = SignedIdentity::new("a shared secret", at(1_000));
        assert_eq!(id.verify("alice"), Err(Rejected::Invalid));
        assert_eq!(id.verify("alice.deadbeef"), Err(Rejected::Invalid));
        assert_eq!(id.verify(""), Err(Rejected::Missing));
    }

    /// Nor can it borrow somebody else's: the payload is what is signed, so editing the name
    /// invalidates the tag.
    #[test]
    fn a_credential_cannot_be_edited_into_another_actors() {
        let id = SignedIdentity::new("a shared secret", at(1_000));
        let token = id.mint("alice", 2_000, &[("role", "reader")]);
        let forged = token.replacen("alice", "admin", 1);
        assert_eq!(id.verify(&forged), Err(Rejected::Invalid));

        // Nor into a better claim.
        let escalated = token.replacen("role=reader", "role=admin!", 1);
        assert_eq!(id.verify(&escalated), Err(Rejected::Invalid));
    }

    #[test]
    fn a_credential_for_another_secret_does_not_verify() {
        let mint = SignedIdentity::new("one secret", at(1_000));
        let check = SignedIdentity::new("another secret", at(1_000));
        let token = mint.mint("alice", 2_000, &[]);
        assert_eq!(check.verify(&token), Err(Rejected::Invalid));
    }

    /// Expiry is read from the injected clock, so this is a statement about an instant rather than
    /// about how long the test took to run.
    #[test]
    fn a_credential_expires_against_the_clock_it_was_given() {
        let clock = at(1_000);
        let id = SignedIdentity::new("a shared secret", clock.clone());
        let token = id.mint("alice", 2_000, &[]);
        assert!(id.verify(&token).is_ok());
        clock.set(2_000);
        assert_eq!(id.verify(&token), Err(Rejected::Expired));
    }

    /// A client is told it was refused and not which of the three ways, because the difference is
    /// useful to an attacker and to nobody else.
    #[test]
    fn the_client_is_not_told_which_refusal_it_was() {
        for r in [Rejected::Missing, Rejected::Invalid, Rejected::Expired] {
            assert_eq!(r.message(), "unauthenticated");
        }
        assert_ne!(Rejected::Missing.reason(), Rejected::Invalid.reason());
    }
}
