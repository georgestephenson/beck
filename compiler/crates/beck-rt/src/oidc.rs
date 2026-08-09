//! An OpenID Connect **relying party** — the half of [`docs/10`](../../../../../docs/10-decisions.md)
//! D6 that [`48`](../../../../../docs/48-identity-report.md) §48.5 named as unbuilt.
//!
//! [`crate::identity`] made an actor a decision of the runtime and offered two providers, both of
//! which the process can also *mint* for: `DevIdentity` believes a claim and `SignedIdentity`
//! verifies a shared secret. Neither suits a public identity provider, because a process that can
//! verify a credential it could also have issued cannot tell "this user authenticated" from "this
//! process said so". This module is the asymmetric answer: the signature is checked against a
//! **public** key fetched from the issuer, so the only thing that can produce a credential is the
//! issuer.
//!
//! # What is here
//!
//! * **Discovery** — `{issuer}/.well-known/openid-configuration`, which supplies the authorization,
//!   token and JWKS endpoints so an operator configures one URL rather than four.
//! * **A key set**, fetched over TLS, cached, and refetched when a `kid` misses.
//! * **ID-token verification** — RS256/384/512, PS256/384/512, ES256 and ES384, and *nothing else*:
//!   `none` and the HMAC family are refused by name, because "verify with whatever the token says"
//!   is how a relying party is talked into treating a public key as a shared secret.
//! * **The claim checks**, all of them, in one place: issuer, audience, authorized party, expiry,
//!   not-before, and the nonce when there is a nonce to check.
//! * **The authorization-code flow with PKCE**, so a browser can obtain a token in the first place.
//!
//! # Where the trust actually comes from
//!
//! Two links, and both are load-bearing. The signature says the *issuer* produced the token. TLS
//! says the key set came from the issuer — there is nothing else protecting it, which is why the
//! issuer must be an `https` URL and why there is no flag to relax that. [`crate::outbound`] is
//! what makes the second link real, and its own tests are where a handshake is actually performed:
//! this module is tested against a scripted [`beck_core::net::Outbound`], because a relying party
//! tested against a server written beside it tests agreement with itself
//! ([`docs/84`](../../../../../docs/84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5).
//!
//! # What is deliberately not here
//!
//! No **session of Beck's own**: the cookie the flow sets *is* the ID token, so a session lasts as
//! long as the issuer said it should and no longer. That makes token refresh unnecessary rather
//! than missing — there is no local session to keep alive — and it makes logout the deletion of one
//! cookie. §95.6 of the report says what it costs: a user is sent back to the issuer when the token
//! expires, and an issuer that mints five-minute tokens will do that every five minutes.
//!
//! No **UserInfo request**: the claims are the ID token's. `identity = managed()` is built and is
//! `beck-infra`'s (§95.10) — what reaches here from it is one thing, [`Config::in_cluster`], and its
//! field says what it costs.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use aws_lc_rs::signature::{
    self, EcdsaVerificationAlgorithm, RsaParameters, RsaPublicKeyComponents, UnparsedPublicKey,
};
use beck_core::clock::Clock;
use beck_core::net::{Outbound, Request};

use crate::identity::{Actor, Identity, Rejected};

/// How long a fetched key set is used before it is fetched again.
///
/// Five minutes. Short enough that a rotated key is picked up without anybody doing anything, long
/// enough that a Beck process is not a load generator against its own identity provider — and the
/// *interesting* case is not this one anyway: a token signed by a key the set does not carry
/// triggers a refetch by itself, so this interval only decides how quickly a **retired** key stops
/// being accepted.
pub const REFRESH_EVERY_MS: i64 = 5 * 60 * 1_000;

/// The floor between two key-set fetches, whatever asks for one.
///
/// A token naming an unknown `kid` schedules a refetch, and a `kid` is a string an anonymous client
/// chooses — so without a floor, "verify this token" would be a request that makes a Beck process
/// call its identity provider (§43.1's A2, one hop further out). Ten seconds.
pub const REFETCH_FLOOR_MS: i64 = 10_000;

/// How far the clock may be wrong before a token is refused for it.
///
/// Sixty seconds, in **both** directions: a token from an issuer whose clock is a minute ahead is
/// not yet valid, and one whose clock is a minute behind has already expired. This is the number
/// every OIDC library has and none of them explain; it is here because a distributed system without
/// one refuses valid tokens on a machine whose NTP has drifted, and because the alternative to
/// choosing it is choosing it accidentally.
pub const CLOCK_SKEW_MS: i64 = 60_000;

/// How long a login may take between `/auth/login` and `/auth/callback`.
pub const LOGIN_WINDOW_MS: i64 = 10 * 60 * 1_000;

/// Claims that describe the **token** rather than the person, and never reach the program.
///
/// A program reads `session.claims` to decide what somebody may do. `exp` is not something anybody
/// may do; neither is `at_hash`. Excluding them is not a security control — the token is verified
/// either way — it is the difference between a map of an identity and a dump of a wire format.
const PROTOCOL_CLAIMS: &[&str] = &[
    "aud", "azp", "at_hash", "c_hash", "exp", "iat", "iss", "jti", "nbf", "nonce", "typ",
];

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

/// What an operator states. Everything else is discovered.
#[derive(Clone, Debug)]
pub struct Config {
    /// The issuer, as an `https` URL. It is both what is fetched and what every token's `iss` is
    /// compared against, which is why there is one field rather than two.
    pub issuer: String,
    pub client_id: String,
    /// `None` is a public client, which is what a browser-facing app with PKCE is. A confidential
    /// client authenticates to the token endpoint with this.
    pub client_secret: Option<String>,
    /// Where the issuer sends the browser back. Registered with the issuer, so it is stated rather
    /// than derived from whatever `Host` a request happened to carry.
    pub redirect_uri: String,
    /// The scopes asked for. `openid` is mandatory and is added if it is missing.
    pub scopes: String,
    /// Which claim names the actor. `sub` is the only one an issuer guarantees is stable and
    /// unique, which is why it is the default and why choosing another is a decision.
    pub actor_claim: String,
    /// Whether the issuer may be reached over a plaintext hop.
    ///
    /// **False except for a provider this deployment provisioned.** An external issuer must be
    /// `https`, because the key set has no integrity protection but the transport. A *managed* one
    /// is a `Service` §6.5 emitted, in the application's own namespace, reachable only through a
    /// NetworkPolicy §6.5 wrote — so what protects the key set there is the policy, and §6.5's
    /// gateway is where TLS is terminated for everything that crosses a network anybody else is on
    /// ([`docs/95`](../../../../../docs/95-oidc-relying-party-report.md) §95.10).
    ///
    /// Private, and set by [`Config::in_cluster`] rather than assignable: a `pub` bool here would
    /// be the flag §95.2 says does not exist.
    in_cluster: bool,
}

impl Config {
    /// A relying party to somebody else's identity provider. The issuer must be `https`.
    pub fn new(issuer: &str, client_id: &str, redirect_uri: &str) -> Config {
        Config {
            issuer: issuer.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
            client_secret: None,
            redirect_uri: redirect_uri.to_string(),
            scopes: "openid profile email".to_string(),
            actor_claim: "sub".to_string(),
            in_cluster: false,
        }
    }

    /// A relying party to a provider **this deployment provisioned**, reached inside one namespace.
    ///
    /// A second constructor rather than a field, so that the trust story is chosen by name at the
    /// one place that knows which of the two this is — `identity = managed()` in the program, read
    /// by `beck run`. See [`Config::in_cluster`]'s field for what it costs.
    pub fn in_cluster(issuer: &str, client_id: &str, redirect_uri: &str) -> Config {
        Config {
            in_cluster: true,
            ..Config::new(issuer, client_id, redirect_uri)
        }
    }

    /// Whether this relying party's issuer is one the deployment provisioned.
    pub fn is_in_cluster(&self) -> bool {
        self.in_cluster
    }
}

/// The endpoints, as the issuer published them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

// ---------------------------------------------------------------------------------------------
// The relying party
// ---------------------------------------------------------------------------------------------

/// Why a token was refused, in the operator's words.
///
/// [`Rejected`] is what the *client* gets and has three values on purpose ([`docs/48`](../../../../../docs/48-identity-report.md)
/// §48.3). This is the other half of that decision: an operator debugging a login needs to know
/// that the audience was wrong, and telling the client would be telling an attacker which of a
/// dozen checks to work on next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub client: Rejected,
    pub why: String,
}

impl Refusal {
    fn invalid(why: impl Into<String>) -> Refusal {
        Refusal {
            client: Rejected::Invalid,
            why: why.into(),
        }
    }

    fn expired(why: impl Into<String>) -> Refusal {
        Refusal {
            client: Rejected::Expired,
            why: why.into(),
        }
    }
}

#[derive(Debug, Default)]
struct KeySet {
    keys: Vec<Key>,
    fetched_at_millis: i64,
}

/// A relying party: one issuer, one client, one key set.
#[derive(Debug)]
pub struct RelyingParty {
    config: Config,
    clock: Arc<dyn Clock>,
    http: Arc<dyn Outbound>,
    provider: RwLock<Option<Provider>>,
    keys: RwLock<KeySet>,
    /// Set when a token named a `kid` the set does not carry. Read by the refresher rather than
    /// acted on here: verification is on the connection path and must not make a network call.
    stale: AtomicBool,
    /// When the last fetch was attempted, successful or not — [`REFETCH_FLOOR_MS`]'s counter.
    last_attempt_millis: AtomicI64,
    /// The key the login transaction cookie is sealed with. Random per process: a restart
    /// invalidates logins that are in flight, which is ten minutes of nobody.
    seal: [u8; 32],
}

impl RelyingParty {
    pub fn new(config: Config, clock: Arc<dyn Clock>, http: Arc<dyn Outbound>) -> RelyingParty {
        let mut seal = [0u8; 32];
        aws_lc_rs::rand::fill(&mut seal).expect("the system random source answers");
        RelyingParty {
            config,
            clock,
            http,
            provider: RwLock::new(None),
            keys: RwLock::new(KeySet::default()),
            stale: AtomicBool::new(false),
            last_attempt_millis: AtomicI64::new(i64::MIN),
            seal,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The endpoints, once discovery has run.
    pub fn provider(&self) -> Option<Provider> {
        self.provider.read().expect("not poisoned").clone()
    }

    pub fn key_count(&self) -> usize {
        self.keys.read().expect("not poisoned").keys.len()
    }

    /// Fetch the discovery document and the key set.
    ///
    /// Called once at startup — a process that cannot reach its identity provider should say so
    /// then rather than when the first person tries to log in — and then on
    /// [`REFRESH_EVERY_MS`], and then whenever a token names an unknown key.
    pub fn refresh(&self) -> Result<(), String> {
        let now = self.clock.now_millis();
        self.last_attempt_millis.store(now, Ordering::Relaxed);
        let provider = match self.provider() {
            Some(p) => p,
            None => {
                let p = self.discover()?;
                *self.provider.write().expect("not poisoned") = Some(p.clone());
                p
            }
        };
        let body = self.get(&provider.jwks_uri)?;
        let keys = parse_jwks(&body)?;
        if keys.is_empty() {
            return Err(format!(
                "`{}` published a key set with no key this relying party can use",
                provider.jwks_uri
            ));
        }
        *self.keys.write().expect("not poisoned") = KeySet {
            keys,
            fetched_at_millis: now,
        };
        self.stale.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Whether [`RelyingParty::refresh`] is due — the interval has passed, or a token named a key
    /// the set does not carry and the floor between fetches has passed.
    pub fn refresh_due(&self) -> bool {
        let now = self.clock.now_millis();
        let last = self.last_attempt_millis.load(Ordering::Relaxed);
        if now.saturating_sub(last) < REFETCH_FLOOR_MS {
            return false;
        }
        self.stale.load(Ordering::Relaxed)
            || now.saturating_sub(self.keys.read().expect("not poisoned").fetched_at_millis)
                >= REFRESH_EVERY_MS
    }

    fn discover(&self) -> Result<Provider, String> {
        let url = format!("{}/.well-known/openid-configuration", self.config.issuer);
        let body = self.get(&url)?;
        let doc: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("`{url}` is not JSON: {e}"))?;
        let field = |name: &str| -> Result<String, String> {
            doc.get(name)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| format!("`{url}` has no `{name}`"))
        };
        let provider = Provider {
            issuer: field("issuer")?,
            authorization_endpoint: field("authorization_endpoint")?,
            token_endpoint: field("token_endpoint")?,
            jwks_uri: field("jwks_uri")?,
        };
        // The issuer identifier is the one thing in this document that is *also* in every token, so
        // a document whose `issuer` is not the one we asked for is a document about somebody else.
        if provider.issuer.trim_end_matches('/') != self.config.issuer {
            return Err(format!(
                "`{url}` says its issuer is `{}`, which is not `{}`",
                provider.issuer, self.config.issuer
            ));
        }
        // Every endpoint on the issuer's own host, which narrows the egress rule to one name and
        // stops a discovery document moving the token exchange — and the client secret with it —
        // somewhere else. An issuer that splits its endpoints across hosts is not usable here, and
        // that is a limit rather than an oversight (§95.6).
        let host = url_host(&self.config.issuer, self.config.in_cluster)?;
        for (name, endpoint) in [
            ("authorization_endpoint", &provider.authorization_endpoint),
            ("token_endpoint", &provider.token_endpoint),
            ("jwks_uri", &provider.jwks_uri),
        ] {
            let elsewhere = url_host(endpoint, self.config.in_cluster)?;
            if elsewhere != host {
                return Err(format!(
                    "`{name}` is on `{elsewhere}`, and this relying party only reaches `{host}`"
                ));
            }
        }
        Ok(provider)
    }

    fn get(&self, url: &str) -> Result<String, String> {
        let target = Target::parse(url, self.config.in_cluster)?;
        let reply = self
            .http
            .fetch(&Request {
                host: Arc::from(target.host.as_str()),
                port: target.port,
                tls: target.tls,
                method: Arc::from("GET"),
                path: Arc::from(target.path.as_str()),
                headers: vec![(Arc::from("accept"), Arc::from("application/json"))],
                body: Arc::from(""),
            })
            .map_err(|e| format!("`{url}` was not reached: {e:?}"))?;
        if reply.status != 200 {
            return Err(format!("`{url}` answered {}", reply.status));
        }
        Ok(reply.body.to_string())
    }

    // ------------------------------------------------------------------------------ verification

    /// Verify an ID token and say who it is about.
    ///
    /// `nonce` is `Some` exactly once in a token's life — at the callback, where the relying party
    /// still remembers what it asked for. On every later connection there is nothing to compare
    /// against, and pretending otherwise would be a check that always passes.
    pub fn verify_id_token(&self, token: &str, nonce: Option<&str>) -> Result<Verified, Refusal> {
        let (signed, signature, header, payload) = split(token)?;
        let alg = header
            .get("alg")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Refusal::invalid("the token's header names no algorithm"))?;
        let alg = Algorithm::named(alg).ok_or_else(|| {
            Refusal::invalid(format!(
                "`{alg}` is not an algorithm this relying party verifies"
            ))
        })?;
        let kid = header.get("kid").and_then(|v| v.as_str());

        let keys = self.keys.read().expect("not poisoned");
        let candidates: Vec<&Key> = keys
            .keys
            .iter()
            .filter(|k| k.usable_for(alg, kid))
            .collect();
        if candidates.is_empty() {
            // The key set may have rotated under us. Say so to the refresher rather than fetching
            // here: this runs on the connection path, and an anonymous client chooses the `kid`.
            self.stale.store(true, Ordering::Relaxed);
            return Err(Refusal::invalid(match kid {
                Some(kid) => format!("no key `{kid}` for {alg:?} in the issuer's key set"),
                None => format!("no {alg:?} key in the issuer's key set"),
            }));
        }
        if !candidates
            .iter()
            .any(|k| k.verifies(alg, signed.as_bytes(), &signature))
        {
            return Err(Refusal::invalid("the signature does not verify"));
        }
        drop(keys);

        self.check_claims(&payload, nonce)
    }

    fn check_claims(
        &self,
        payload: &serde_json::Value,
        nonce: Option<&str>,
    ) -> Result<Verified, Refusal> {
        let str_claim = |name: &str| payload.get(name).and_then(|v| v.as_str());

        match str_claim("iss") {
            Some(iss) if iss.trim_end_matches('/') == self.config.issuer => {}
            Some(iss) => {
                return Err(Refusal::invalid(format!(
                    "the token is from `{iss}`, not `{}`",
                    self.config.issuer
                )))
            }
            None => return Err(Refusal::invalid("the token names no issuer")),
        }

        // `aud` is a string or an array of them, and a token with several audiences must say which
        // party it is *for* — otherwise a token minted for another client of the same issuer is a
        // token for this one.
        let audiences = audiences(payload);
        if !audiences.iter().any(|a| a == &self.config.client_id) {
            return Err(Refusal::invalid(format!(
                "the token's audience is {audiences:?}, which does not include `{}`",
                self.config.client_id
            )));
        }
        if audiences.len() > 1 {
            match str_claim("azp") {
                Some(azp) if azp == self.config.client_id => {}
                _ => return Err(Refusal::invalid(
                    "the token has several audiences and its authorized party is not this client",
                )),
            }
        }

        let now = self.clock.now_millis();
        let seconds = |name: &str| payload.get(name).and_then(|v| v.as_i64());
        let exp = seconds("exp").ok_or_else(|| Refusal::invalid("the token has no expiry"))?;
        let expires_at = exp.saturating_mul(1_000);
        if now.saturating_sub(CLOCK_SKEW_MS) >= expires_at {
            return Err(Refusal::expired(format!(
                "the token expired at {expires_at} and it is {now}"
            )));
        }
        if let Some(nbf) = seconds("nbf") {
            if now.saturating_add(CLOCK_SKEW_MS) < nbf.saturating_mul(1_000) {
                return Err(Refusal::invalid("the token is not valid yet"));
            }
        }

        if let Some(expected) = nonce {
            match str_claim("nonce") {
                Some(got) if got == expected => {}
                Some(_) => return Err(Refusal::invalid("the token replies to another login")),
                None => return Err(Refusal::invalid("the token carries no nonce")),
            }
        }

        let subject = str_claim(&self.config.actor_claim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Refusal::invalid(format!(
                    "the token has no `{}` to name an actor with",
                    self.config.actor_claim
                ))
            })?
            .to_string();

        Ok(Verified {
            subject,
            claims: person_claims(payload),
            expires_at_millis: expires_at,
        })
    }

    // ------------------------------------------------------------------------------- the flow

    /// Where to send a browser that wants to log in, and the cookie that remembers what we asked.
    ///
    /// `return_to` is a **path**, checked to be one: a redirect target a client supplies is an open
    /// redirect if it can name a host.
    pub fn begin_login(&self, return_to: &str) -> Result<Login, String> {
        let provider = self
            .provider()
            .ok_or_else(|| "the identity provider has not been discovered yet".to_string())?;
        let return_to = if return_to.starts_with('/') && !return_to.starts_with("//") {
            return_to
        } else {
            "/"
        };
        let state = random_token();
        let nonce = random_token();
        let verifier = random_token();
        let challenge = beck_core::digest::base64_encode_bytes(
            aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, verifier.as_bytes()).as_ref(),
        );

        let scopes = if self.config.scopes.split_whitespace().any(|s| s == "openid") {
            self.config.scopes.clone()
        } else {
            format!("openid {}", self.config.scopes)
        };
        let query = [
            ("response_type", "code"),
            ("client_id", &self.config.client_id),
            ("redirect_uri", &self.config.redirect_uri),
            ("scope", &scopes),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
        let joiner = if provider.authorization_endpoint.contains('?') {
            '&'
        } else {
            '?'
        };

        Ok(Login {
            url: format!("{}{joiner}{query}", provider.authorization_endpoint),
            transaction: self.seal_transaction(&Transaction {
                state,
                nonce,
                verifier,
                return_to: return_to.to_string(),
                expires_at_millis: self.clock.now_millis() + LOGIN_WINDOW_MS,
            }),
        })
    }

    /// The browser is back. Check what it brought, swap the code for a token, and verify it.
    ///
    /// The result is the ID token itself: it is what the session cookie carries, so that every
    /// later connection re-verifies the *issuer's* signature rather than one this process made up.
    pub fn complete_login(&self, query: &str, transaction: &str) -> Result<Completion, String> {
        let tx = self.open_transaction(transaction)?;
        let params = query_params(query);
        let got = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };

        if let Some(error) = got("error") {
            return Err(format!(
                "the identity provider refused the login: {error}{}",
                got("error_description")
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ));
        }
        let state = got("state").ok_or_else(|| "the reply carries no state".to_string())?;
        // Constant-time, because this is a comparison of a secret against something an attacker
        // supplies and can vary one byte at a time.
        if !beck_core::digest::same(&state, &tx.state) {
            return Err("the reply's state is not the one this login asked for".to_string());
        }
        let code = got("code").ok_or_else(|| "the reply carries no code".to_string())?;

        let id_token = self.exchange(&code, &tx.verifier)?;
        let verified = self
            .verify_id_token(&id_token, Some(&tx.nonce))
            .map_err(|r| r.why)?;
        Ok(Completion {
            id_token,
            verified,
            return_to: tx.return_to,
        })
    }

    fn exchange(&self, code: &str, verifier: &str) -> Result<String, String> {
        let provider = self
            .provider()
            .ok_or_else(|| "the identity provider has not been discovered yet".to_string())?;
        let target = Target::parse(&provider.token_endpoint, self.config.in_cluster)?;

        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", self.config.redirect_uri.clone()),
            ("client_id", self.config.client_id.clone()),
            ("code_verifier", verifier.to_string()),
        ];
        let mut headers = vec![
            (
                Arc::from("content-type"),
                Arc::from("application/x-www-form-urlencoded"),
            ),
            (Arc::from("accept"), Arc::from("application/json")),
        ];
        // `client_secret_basic` when there is a secret, PKCE alone when there is not — which is a
        // public client, and is what a browser-facing app is.
        if let Some(secret) = &self.config.client_secret {
            let basic = beck_core::digest::base64_encode(&format!(
                "{}:{secret}",
                percent_encode(&self.config.client_id)
            ));
            headers.push((
                Arc::from("authorization"),
                Arc::from(format!("Basic {basic}")),
            ));
        } else {
            form.retain(|(k, _)| *k != "client_secret");
        }
        let body = form
            .iter()
            .map(|(k, v)| format!("{k}={}", percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        headers.push((
            Arc::from("content-length"),
            Arc::from(body.len().to_string()),
        ));

        let reply = self
            .http
            .fetch(&Request {
                host: Arc::from(target.host.as_str()),
                port: target.port,
                tls: target.tls,
                method: Arc::from("POST"),
                path: Arc::from(target.path.as_str()),
                headers,
                body: Arc::from(body.as_str()),
            })
            .map_err(|e| format!("the token endpoint was not reached: {e:?}"))?;
        if reply.status != 200 {
            // The body of a failed token exchange names the client and sometimes the code; the
            // status is what an operator needs and the rest is not ours to log.
            return Err(format!("the token endpoint answered {}", reply.status));
        }
        let doc: serde_json::Value = serde_json::from_str(&reply.body)
            .map_err(|e| format!("the token endpoint did not answer JSON: {e}"))?;
        doc.get("id_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "the token endpoint's answer carries no id_token".to_string())
    }

    // --------------------------------------------------------------------- the sealed transaction

    fn seal_transaction(&self, tx: &Transaction) -> String {
        let payload = format!(
            "{}.{}.{}.{}.{}",
            tx.state,
            tx.nonce,
            tx.verifier,
            tx.expires_at_millis,
            beck_core::digest::base64_encode(&tx.return_to)
        );
        let mac = blake3::keyed_hash(&self.seal, payload.as_bytes());
        format!("{payload}.{}", mac.to_hex())
    }

    fn open_transaction(&self, sealed: &str) -> Result<Transaction, String> {
        let (payload, mac) = sealed
            .rsplit_once('.')
            .ok_or_else(|| "the login has no cookie to check against".to_string())?;
        let expected = blake3::keyed_hash(&self.seal, payload.as_bytes());
        let given: blake3::Hash = mac
            .parse()
            .map_err(|_| "the login cookie is malformed".to_string())?;
        if expected != given {
            return Err("the login cookie was not sealed by this process".to_string());
        }
        let parts: Vec<&str> = payload.split('.').collect();
        let [state, nonce, verifier, expiry, return_to] = parts[..] else {
            return Err("the login cookie is malformed".to_string());
        };
        let expires_at_millis: i64 = expiry
            .parse()
            .map_err(|_| "the login cookie is malformed".to_string())?;
        if self.clock.now_millis() >= expires_at_millis {
            return Err("the login took longer than the window allows".to_string());
        }
        Ok(Transaction {
            state: state.to_string(),
            nonce: nonce.to_string(),
            verifier: verifier.to_string(),
            return_to: beck_core::digest::base64_decode(return_to)
                .map_err(|_| "the login cookie is malformed".to_string())?,
            expires_at_millis,
        })
    }
}

/// What a verified ID token said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub subject: String,
    pub claims: BTreeMap<Arc<str>, Arc<str>>,
    pub expires_at_millis: i64,
}

/// Where to send the browser, and what to remember while it is gone.
#[derive(Clone, Debug)]
pub struct Login {
    pub url: String,
    /// The sealed transaction, for the cookie. It carries the state, the nonce and the PKCE
    /// verifier — so the relying party holds no per-login memory and a login cannot be a way to
    /// make it allocate.
    pub transaction: String,
}

/// The browser came back and the token verified.
#[derive(Clone, Debug)]
pub struct Completion {
    /// The session cookie's value. It is the issuer's token, not one this process made.
    pub id_token: String,
    pub verified: Verified,
    pub return_to: String,
}

#[derive(Clone, Debug)]
struct Transaction {
    state: String,
    nonce: String,
    verifier: String,
    return_to: String,
    expires_at_millis: i64,
}

impl Identity for RelyingParty {
    /// The claim is the ID token, from the session cookie or from the `hello` frame.
    ///
    /// There is no nonce here — see [`RelyingParty::verify_id_token`]. Everything else is checked
    /// on **every** connection rather than once at login, which is what makes the session's
    /// lifetime the issuer's decision.
    fn verify(&self, claim: &str) -> Result<Actor, Rejected> {
        if claim.is_empty() {
            return Err(Rejected::Missing);
        }
        match self.verify_id_token(claim, None) {
            Ok(v) => Ok(Actor::verified(&v.subject, v.claims)),
            Err(refusal) => {
                // Specific to the operator, coarse to the client (`docs/48` §48.3). This is the
                // only place the distinction between a dozen checks survives.
                tracing::warn!(why = %refusal.why, "an id token did not verify");
                Err(refusal.client)
            }
        }
    }

    fn kind(&self) -> &'static str {
        "oidc"
    }

    fn login(&self) -> Option<&RelyingParty> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------------------------
// JOSE
// ---------------------------------------------------------------------------------------------

/// The algorithms this relying party verifies, and by construction the only ones.
///
/// The absences are the point. `none` is an algorithm in the JWS registry and means "there is no
/// signature"; the HMAC family is symmetric, so a relying party that accepted `HS256` could be
/// handed a token signed with the issuer's own **public** key as the shared secret. Both are the
/// canonical way a relying party is broken, and both are refused by not being here rather than by
/// a check somebody has to remember to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Algorithm {
    Rs256,
    Rs384,
    Rs512,
    Ps256,
    Ps384,
    Ps512,
    Es256,
    Es384,
}

impl Algorithm {
    fn named(alg: &str) -> Option<Algorithm> {
        Some(match alg {
            "RS256" => Algorithm::Rs256,
            "RS384" => Algorithm::Rs384,
            "RS512" => Algorithm::Rs512,
            "PS256" => Algorithm::Ps256,
            "PS384" => Algorithm::Ps384,
            "PS512" => Algorithm::Ps512,
            "ES256" => Algorithm::Es256,
            "ES384" => Algorithm::Es384,
            _ => return None,
        })
    }

    fn family(self) -> Family {
        match self {
            Algorithm::Es256 => Family::Ec("P-256"),
            Algorithm::Es384 => Family::Ec("P-384"),
            _ => Family::Rsa,
        }
    }

    fn rsa(self) -> &'static RsaParameters {
        match self {
            Algorithm::Rs256 => &signature::RSA_PKCS1_2048_8192_SHA256,
            Algorithm::Rs384 => &signature::RSA_PKCS1_2048_8192_SHA384,
            Algorithm::Rs512 => &signature::RSA_PKCS1_2048_8192_SHA512,
            Algorithm::Ps256 => &signature::RSA_PSS_2048_8192_SHA256,
            Algorithm::Ps384 => &signature::RSA_PSS_2048_8192_SHA384,
            Algorithm::Ps512 => &signature::RSA_PSS_2048_8192_SHA512,
            // Not reachable: the caller matches on `family` first, and an EC algorithm has no RSA
            // parameters to give. Returning the narrowest thing rather than panicking, so a future
            // caller that gets this wrong fails a signature instead of the process.
            Algorithm::Es256 | Algorithm::Es384 => &signature::RSA_PKCS1_2048_8192_SHA256,
        }
    }

    /// The **fixed** ECDSA encoding, which is what JWS uses: `r` and `s` concatenated at the
    /// curve's width. The ASN.1 encoding is a different byte string for the same signature, and a
    /// verifier that accepted both would accept two spellings of one token.
    fn ecdsa(self) -> &'static EcdsaVerificationAlgorithm {
        match self {
            Algorithm::Es384 => &signature::ECDSA_P384_SHA384_FIXED,
            _ => &signature::ECDSA_P256_SHA256_FIXED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Rsa,
    Ec(&'static str),
}

/// One key from the issuer's set.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    kid: Option<String>,
    /// The `alg` the key declares, if it declares one. A key that names an algorithm may only be
    /// used for that one — otherwise an issuer publishing one key for signing and one for
    /// encryption is an issuer whose encryption key verifies signatures.
    alg: Option<String>,
    material: Material,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Material {
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    /// `0x04 || x || y`, the uncompressed point encoding both aws-lc-rs and the JWK spec agree on.
    Ec {
        curve: &'static str,
        point: Vec<u8>,
    },
}

impl Key {
    fn usable_for(&self, alg: Algorithm, kid: Option<&str>) -> bool {
        // A token that names a `kid` may only be verified by that key. A token that names none is
        // offered every key of the right shape, which is what a set with one key needs.
        if let Some(kid) = kid {
            if self.kid.as_deref() != Some(kid) {
                return false;
            }
        }
        if let Some(declared) = &self.alg {
            if Algorithm::named(declared) != Some(alg) {
                return false;
            }
        }
        match (&self.material, alg.family()) {
            (Material::Rsa { .. }, Family::Rsa) => true,
            (Material::Ec { curve, .. }, Family::Ec(wanted)) => *curve == wanted,
            _ => false,
        }
    }

    fn verifies(&self, alg: Algorithm, message: &[u8], signature: &[u8]) -> bool {
        match &self.material {
            Material::Rsa { n, e } => RsaPublicKeyComponents { n, e }
                .verify(alg.rsa(), message, signature)
                .is_ok(),
            Material::Ec { point, .. } => UnparsedPublicKey::new(alg.ecdsa(), point)
                .verify(message, signature)
                .is_ok(),
        }
    }
}

/// The signing input, the signature, and the two decoded segments.
///
/// The signing input is the token's own bytes — `header.payload` exactly as received — rather than
/// a re-encoding of what was parsed out of them. Re-encoding is how a verifier ends up checking a
/// signature over something the issuer did not sign.
fn split(token: &str) -> Result<(&str, Vec<u8>, serde_json::Value, serde_json::Value), Refusal> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Refusal::invalid(
            "a JWS compact serialization has exactly three segments",
        ));
    };
    let signed = &token[..header.len() + 1 + payload.len()];
    let signature = beck_core::digest::base64_decode_bytes(signature)
        .map_err(|e| Refusal::invalid(format!("the signature segment is not base64url: {e}")))?;
    let decode = |segment: &str, what: &str| -> Result<serde_json::Value, Refusal> {
        let bytes = beck_core::digest::base64_decode_bytes(segment)
            .map_err(|e| Refusal::invalid(format!("the {what} is not base64url: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Refusal::invalid(format!("the {what} is not JSON: {e}")))
    };
    Ok((
        signed,
        signature,
        decode(header, "header")?,
        decode(payload, "payload")?,
    ))
}

fn parse_jwks(body: &str) -> Result<Vec<Key>, String> {
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("the key set is not JSON: {e}"))?;
    let keys = doc
        .get("keys")
        .and_then(|k| k.as_array())
        .ok_or_else(|| "the key set has no `keys` array".to_string())?;
    // A key this build cannot use is skipped rather than fatal: an issuer publishing an Ed25519
    // key beside its RSA one is an issuer doing nothing wrong, and refusing the whole set would
    // make somebody else's roadmap our outage.
    Ok(keys.iter().filter_map(parse_jwk).collect())
}

fn parse_jwk(jwk: &serde_json::Value) -> Option<Key> {
    let field = |name: &str| jwk.get(name).and_then(|v| v.as_str());
    // `use` is optional; when it is present and says `enc`, this is an encryption key.
    if matches!(field("use"), Some(u) if u != "sig") {
        return None;
    }
    let bytes = |name: &str| beck_core::digest::base64_decode_bytes(field(name)?).ok();
    let material = match field("kty")? {
        "RSA" => Material::Rsa {
            n: bytes("n")?,
            e: bytes("e")?,
        },
        "EC" => {
            let curve = match field("crv")? {
                "P-256" => "P-256",
                "P-384" => "P-384",
                _ => return None,
            };
            let (x, y) = (bytes("x")?, bytes("y")?);
            // Each coordinate is left-padded to the curve's width: a JWK writes them fixed-width,
            // and a shorter one is a key we cannot assemble rather than one to guess at.
            let width = if curve == "P-256" { 32 } else { 48 };
            if x.len() != width || y.len() != width {
                return None;
            }
            let mut point = Vec::with_capacity(1 + 2 * width);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            Material::Ec { curve, point }
        }
        _ => return None,
    };
    Some(Key {
        kid: field("kid").map(|s| s.to_string()),
        alg: field("alg").map(|s| s.to_string()),
        material,
    })
}

fn audiences(payload: &serde_json::Value) -> Vec<String> {
    match payload.get("aud") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(xs)) => xs
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The claims that describe a person, as strings.
///
/// Scalars only: a claim whose value is an object is a shape a `map[Str, Str]` cannot hold, and
/// flattening it would invent names the issuer never used. An array is joined with a space, which
/// is what `scope` and `groups` already are in every issuer that emits them.
fn person_claims(payload: &serde_json::Value) -> BTreeMap<Arc<str>, Arc<str>> {
    let mut out = BTreeMap::new();
    let Some(fields) = payload.as_object() else {
        return out;
    };
    for (name, value) in fields {
        if PROTOCOL_CLAIMS.contains(&name.as_str()) {
            continue;
        }
        let rendered = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Array(xs) => {
                let parts: Vec<&str> = xs.iter().filter_map(|x| x.as_str()).collect();
                if parts.len() != xs.len() {
                    continue;
                }
                parts.join(" ")
            }
            _ => continue,
        };
        out.insert(Arc::from(name.as_str()), Arc::from(rendered.as_str()));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// URLs, in the small amount this needs
// ---------------------------------------------------------------------------------------------

/// Where an `https` URL points, in the three things [`beck_core::net::Request`] asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    path: String,
    /// Whether the exchange goes inside a TLS session. Always true for an external issuer.
    tls: bool,
}

impl Target {
    /// `https` only, and a host a `net.out` atom could have named.
    ///
    /// Both refusals are the same one from two sides. The key set is only trustworthy because TLS
    /// says who sent it, so an `http` issuer is not a weaker configuration but a different feature.
    /// And a host with a userinfo section or a port in the name is a host §6.5's egress rule cannot
    /// be written from.
    fn parse(url: &str, in_cluster: bool) -> Result<Target, String> {
        let (rest, tls) = match url.strip_prefix("https://") {
            Some(rest) => (rest, true),
            None => match url.strip_prefix("http://").filter(|_| in_cluster) {
                Some(rest) => (rest, false),
                None => {
                    return Err(format!(
                        "`{url}` is not an https URL, and this one has to be"
                    ))
                }
            },
        };
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err(format!("`{url}` carries credentials in its authority"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (
                h,
                p.parse::<u16>()
                    .map_err(|_| format!("`{url}` has no readable port"))?,
            ),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if !beck_core::net::is_nameable_host(host) {
            return Err(format!(
                "`{host}` is not a host an egress rule could be written for"
            ));
        }
        Ok(Target {
            host: host.to_string(),
            port,
            path: path.to_string(),
            tls,
        })
    }
}

fn url_host(url: &str, in_cluster: bool) -> Result<String, String> {
    Ok(Target::parse(url, in_cluster)?.host)
}

/// RFC 3986's unreserved set, and everything else escaped.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte))
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
                continue;
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a=1&b=2`, decoded. A repeated name keeps every value, and the reader takes the first — which
/// is the behaviour that makes `?code=good&code=evil` unambiguous.
///
/// Public because a `application/x-www-form-urlencoded` body has the same shape, and because the
/// harness reads a query the same way the edge does: two readers would be two sets of rules about
/// what `+` means.
pub fn query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// 256 bits, base64url. Used for the state, the nonce and the PKCE verifier — all three are values
/// an attacker must not be able to guess, and one function is one place to be right about it.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random source answers");
    beck_core::digest::base64_encode_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_https_url_with_a_nameable_host_is_a_target() {
        assert_eq!(
            Target::parse("https://login.acme.com/authorize", false),
            Ok(Target {
                host: "login.acme.com".into(),
                port: 443,
                path: "/authorize".into(),
                tls: true,
            })
        );
        assert_eq!(
            Target::parse("https://login.acme.com:8443", false),
            Ok(Target {
                host: "login.acme.com".into(),
                port: 8443,
                path: "/".into(),
                tls: true,
            })
        );
        // The three refusals, each for its own reason.
        assert!(Target::parse("http://login.acme.com/", false).is_err());
        assert!(Target::parse("https://user:pw@login.acme.com/", false).is_err());
        assert!(Target::parse("https://not a host/", false).is_err());
    }

    /// A provider this deployment provisioned is reached inside one namespace, so `http` is
    /// admissible there and **only** there — and it is the declaration that decides, not the URL.
    #[test]
    fn a_plaintext_issuer_is_a_target_only_for_a_provisioned_provider() {
        assert_eq!(
            Target::parse("http://todo-identity:8080/realms/todo", true),
            Ok(Target {
                host: "todo-identity".into(),
                port: 8080,
                path: "/realms/todo".into(),
                tls: false,
            })
        );
        // The same URL, for a relying party that did not provision its provider.
        assert!(Target::parse("http://todo-identity:8080/realms/todo", false).is_err());
        // And the relaxation is about the transport only: a host an egress rule could not name is
        // still refused, in cluster or out.
        assert!(Target::parse("http://user:pw@todo-identity/", true).is_err());
        assert!(Target::parse("http://not a host/", true).is_err());
    }

    #[test]
    fn a_query_is_read_the_way_a_browser_wrote_it() {
        assert_eq!(
            query_params("code=a%2Fb&state=x+y"),
            vec![
                ("code".to_string(), "a/b".to_string()),
                ("state".to_string(), "x y".to_string())
            ]
        );
        assert_eq!(query_params(""), Vec::new());
    }

    #[test]
    fn the_algorithms_that_are_not_here_are_the_point() {
        assert_eq!(Algorithm::named("RS256"), Some(Algorithm::Rs256));
        assert_eq!(Algorithm::named("ES256"), Some(Algorithm::Es256));
        // The two that break a relying party, refused by not existing.
        assert_eq!(Algorithm::named("none"), None);
        assert_eq!(Algorithm::named("HS256"), None);
        assert_eq!(Algorithm::named("EdDSA"), None);
    }

    #[test]
    fn a_key_set_keeps_what_it_understands_and_drops_the_rest() {
        let set = parse_jwks(
            r#"{"keys":[
                {"kty":"RSA","kid":"a","n":"AQAB","e":"AQAB"},
                {"kty":"RSA","kid":"enc","use":"enc","n":"AQAB","e":"AQAB"},
                {"kty":"OKP","kid":"ed","crv":"Ed25519","x":"AQAB"}
            ]}"#,
        )
        .expect("a key set");
        assert_eq!(set.len(), 1, "{set:?}");
        assert_eq!(set[0].kid.as_deref(), Some("a"));
    }

    #[test]
    fn a_claim_map_carries_the_person_and_not_the_envelope() {
        let payload = serde_json::json!({
            "sub": "ana", "email": "ana@acme.com", "email_verified": true,
            "groups": ["admin", "billing"], "address": {"country": "GB"},
            "iss": "https://login.acme.com", "exp": 1, "nonce": "n"
        });
        let claims = person_claims(&payload);
        assert_eq!(claims.get("sub").map(|s| s.as_ref()), Some("ana"));
        assert_eq!(
            claims.get("email_verified").map(|s| s.as_ref()),
            Some("true")
        );
        assert_eq!(
            claims.get("groups").map(|s| s.as_ref()),
            Some("admin billing")
        );
        assert!(!claims.contains_key("iss"), "{claims:?}");
        assert!(!claims.contains_key("exp"), "{claims:?}");
        assert!(!claims.contains_key("nonce"), "{claims:?}");
        // An object has no rendering a `map[Str, Str]` can hold, so it is absent rather than
        // flattened into names the issuer never used.
        assert!(!claims.contains_key("address"), "{claims:?}");
    }
}
