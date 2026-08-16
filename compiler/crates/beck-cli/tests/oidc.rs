//! The OIDC relying party, driven against an issuer that is not it.
//!
//! [`docs/48`](../../../../docs/48-identity-report.md) §48.13 lists "OIDC relying party" as **not
//! built**, and [`docs/43`](../../../../docs/43-threat-model.md) §43.4 carries the absence. This
//! file is the evidence for the claim that replaces it, and its shape is
//! [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10's lesson:
//! a relying party checked against a token this file *made up* would be a test of agreement with
//! itself, so the tokens here are signed by a **key pair generated for the test**, through the same
//! `aws-lc-rs` primitives an issuer uses, and the relying party is given nothing but the public
//! half in a JWKS document.
//!
//! The issuer is a scripted [`beck_core::net::Outbound`] rather than a socket. That is deliberate
//! and it is where the seam pays: the relying party's *only* contact with the outside world is
//! `fetch`, so a fake issuer is a routing table and the TLS underneath it is
//! `beck-rt/src/outbound.rs`'s own tests' business (a real handshake, a real certificate, a real
//! refusal of the wrong name).
//!
//! Every refusal below is a token that differs from a working one in exactly one way.

use std::sync::{Arc, Mutex};

use aws_lc_rs::signature::KeyPair;
use beck_core::clock::ManualClock;
use beck_core::net::{Failure, Outbound, Reply, Request, Stop};
use beck_rt::identity::Identity;
use beck_rt::oidc::{Config, RelyingParty};

mod support;

const ISSUER: &str = "https://login.acme.test";
const CLIENT: &str = "beck-todo";
const REDIRECT: &str = "http://127.0.0.1:8080/auth/callback";
const NOW: i64 = 1_700_000_000_000;

// ---------------------------------------------------------------------------------------------
// An issuer
// ---------------------------------------------------------------------------------------------

/// An identity provider with a signing key, a published key set, and a token endpoint.
struct Issuer {
    key: aws_lc_rs::rsa::KeyPair,
    kid: String,
    /// What `/token` answers next, and what it was asked.
    exchanges: Mutex<Vec<Request>>,
    code_to_nonce: Mutex<Vec<(String, String)>>,
}

impl std::fmt::Debug for Issuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Issuer")
    }
}

impl Issuer {
    fn new() -> Arc<Issuer> {
        let key = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048)
            .expect("a signing key");
        Arc::new(Issuer {
            key,
            kid: "acme-2026".to_string(),
            exchanges: Mutex::new(Vec::new()),
            code_to_nonce: Mutex::new(Vec::new()),
        })
    }

    fn discovery(&self) -> String {
        serde_json::json!({
            "issuer": ISSUER,
            "authorization_endpoint": format!("{ISSUER}/authorize"),
            "token_endpoint": format!("{ISSUER}/token"),
            "jwks_uri": format!("{ISSUER}/jwks"),
        })
        .to_string()
    }

    /// The public half, as a JWK set. `n` and `e` are the modulus and exponent, base64url, exactly
    /// as an issuer publishes them.
    fn jwks(&self) -> String {
        let public = self.key.public_key();
        let n = beck_core::digest::base64_encode_bytes(
            public.modulus().big_endian_without_leading_zero(),
        );
        let e = beck_core::digest::base64_encode_bytes(
            public.exponent().big_endian_without_leading_zero(),
        );
        serde_json::json!({
            "keys": [{"kty": "RSA", "use": "sig", "alg": "RS256", "kid": self.kid, "n": n, "e": e}]
        })
        .to_string()
    }

    /// Sign a JWS over `claims`, with whatever header this test wants.
    fn token(&self, header: serde_json::Value, claims: serde_json::Value) -> String {
        let signing_input = format!(
            "{}.{}",
            b64(&serde_json::to_vec(&header).expect("a header")),
            b64(&serde_json::to_vec(&claims).expect("claims")),
        );
        let mut signature = vec![0u8; self.key.public_modulus_len()];
        self.key
            .sign(
                &aws_lc_rs::signature::RSA_PKCS1_SHA256,
                &aws_lc_rs::rand::SystemRandom::new(),
                signing_input.as_bytes(),
                &mut signature,
            )
            .expect("the issuer signs");
        format!("{signing_input}.{}", b64(&signature))
    }

    /// A token that verifies: the right issuer, the right audience, and an hour to live.
    fn id_token(&self, subject: &str) -> String {
        self.id_token_with(serde_json::json!({
            "iss": ISSUER,
            "aud": CLIENT,
            "sub": subject,
            "exp": (NOW / 1_000) + 3_600,
            "iat": NOW / 1_000,
            "email": format!("{subject}@acme.test"),
            "groups": ["owners"],
        }))
    }

    fn id_token_with(&self, claims: serde_json::Value) -> String {
        self.token(
            serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": self.kid}),
            claims,
        )
    }
}

fn b64(bytes: &[u8]) -> String {
    beck_core::digest::base64_encode_bytes(bytes)
}

/// The issuer, reachable the only way the relying party can reach anything.
#[derive(Debug)]
struct Network(Arc<Issuer>);

impl Outbound for Network {
    /// Nothing to watch: the issuer answers out of memory.
    fn fetch(&self, request: &Request, _stop: &Stop) -> Result<Reply, Failure> {
        assert!(
            request.tls,
            "the relying party made a plaintext request to its issuer: {request:?}"
        );
        assert_eq!(request.host.as_ref(), "login.acme.test");
        let body = match request.path.as_ref() {
            "/.well-known/openid-configuration" => self.0.discovery(),
            "/jwks" => self.0.jwks(),
            "/token" => {
                self.0
                    .exchanges
                    .lock()
                    .expect("not poisoned")
                    .push(request.clone());
                let form = beck_rt::oidc::query_params(&request.body);
                let code = form
                    .iter()
                    .find(|(k, _)| k == "code")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                let nonce = self
                    .0
                    .code_to_nonce
                    .lock()
                    .expect("not poisoned")
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map(|(_, n)| n.clone());
                let Some(nonce) = nonce else {
                    return Ok(Reply {
                        status: 400,
                        headers: Vec::new(),
                        body: Arc::from("{\"error\":\"invalid_grant\"}"),
                    });
                };
                serde_json::json!({
                    "access_token": "not-used-here",
                    "token_type": "Bearer",
                    "id_token": self.0.id_token_with(serde_json::json!({
                        "iss": ISSUER, "aud": CLIENT, "sub": "ana",
                        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000, "nonce": nonce,
                        "email": "ana@acme.test",
                    })),
                })
                .to_string()
            }
            other => {
                return Ok(Reply {
                    status: 404,
                    headers: Vec::new(),
                    body: Arc::from(format!("no {other} here")),
                })
            }
        };
        Ok(Reply {
            status: 200,
            headers: vec![(Arc::from("content-type"), Arc::from("application/json"))],
            body: Arc::from(body.as_str()),
        })
    }
}

fn relying_party(issuer: &Arc<Issuer>, clock: Arc<ManualClock>) -> RelyingParty {
    let party = RelyingParty::new(
        Config::new(ISSUER, CLIENT, REDIRECT),
        clock,
        Arc::new(Network(issuer.clone())),
    );
    party.refresh().expect("discovery and the key set");
    party
}

fn at(millis: i64) -> Arc<ManualClock> {
    Arc::new(ManualClock::at(millis))
}

// ---------------------------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------------------------

#[test]
fn discovery_finds_the_endpoints_and_the_key_set() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    let found = party.provider().expect("the endpoints were discovered");
    assert_eq!(found.token_endpoint, format!("{ISSUER}/token"));
    assert_eq!(party.key_count(), 1);
}

/// The claim this whole module exists for: a token the **issuer** signed names the actor, and
/// nothing this process holds could have produced it.
#[test]
fn a_signed_id_token_names_the_actor_and_carries_its_claims() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    let actor = party
        .verify(&issuer.id_token("ana"))
        .expect("the issuer's token verifies");
    assert_eq!(actor.name(), "ana");
    assert_eq!(
        actor.claims().get("email").map(|s| s.as_ref()),
        Some("ana@acme.test")
    );
    assert_eq!(
        actor.claims().get("groups").map(|s| s.as_ref()),
        Some("owners"),
        "an array claim arrives as the space-joined form every issuer already uses"
    );
    // The envelope's own fields are not the person's.
    assert!(!actor.claims().contains_key("iss"), "{:?}", actor.claims());
    assert!(!actor.claims().contains_key("exp"), "{:?}", actor.claims());
}

/// A relying party that trusted the token's own `alg` would accept a token signed with the
/// issuer's **public** key as an HMAC secret. This is the canonical break, and it is refused by
/// the algorithm not existing rather than by a check somebody remembered to write.
#[test]
fn an_unsigned_or_symmetric_token_is_refused() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));

    let claims = serde_json::json!({
        "iss": ISSUER, "aud": CLIENT, "sub": "attacker",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    });
    let unsigned = format!(
        "{}.{}.",
        b64(&serde_json::to_vec(&serde_json::json!({"alg": "none"})).expect("a header")),
        b64(&serde_json::to_vec(&claims).expect("claims")),
    );
    assert!(party.verify(&unsigned).is_err(), "`alg: none` was accepted");

    // `HS256`, with the issuer's public modulus as the shared secret — the algorithm-confusion
    // attack, written out rather than described.
    let public = issuer.key.public_key();
    let secret = public.modulus().big_endian_without_leading_zero();
    let signing_input = format!(
        "{}.{}",
        b64(
            &serde_json::to_vec(&serde_json::json!({"alg": "HS256", "kid": issuer.kid}))
                .expect("a header")
        ),
        b64(&serde_json::to_vec(&claims).expect("claims")),
    );
    let mac = aws_lc_rs::hmac::sign(
        &aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret),
        signing_input.as_bytes(),
    );
    let confused = format!("{signing_input}.{}", b64(mac.as_ref()));
    assert!(party.verify(&confused).is_err(), "`HS256` was accepted");
}

/// One byte of the payload changed, and the signature no longer covers it.
#[test]
fn an_edited_token_does_not_verify() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    let token = issuer.id_token("ana");
    let mut parts: Vec<String> = token.split('.').map(|s| s.to_string()).collect();
    parts[1] = b64(&serde_json::to_vec(&serde_json::json!({
        "iss": ISSUER, "aud": CLIENT, "sub": "root",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    }))
    .expect("claims"));
    assert!(party.verify(&parts.join(".")).is_err());
}

/// A token another client of the same issuer was given is a valid token — for somebody else.
#[test]
fn a_token_for_another_audience_is_refused() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    let other = issuer.id_token_with(serde_json::json!({
        "iss": ISSUER, "aud": "some-other-app", "sub": "ana",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    }));
    assert!(party.verify(&other).is_err());

    // Several audiences including ours, and no `azp` naming us: still refused, because a token
    // minted *for* another party is not a token for this one.
    let ambiguous = issuer.id_token_with(serde_json::json!({
        "iss": ISSUER, "aud": [CLIENT, "some-other-app"], "sub": "ana",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    }));
    assert!(party.verify(&ambiguous).is_err());

    let authorized = issuer.id_token_with(serde_json::json!({
        "iss": ISSUER, "aud": [CLIENT, "some-other-app"], "azp": CLIENT, "sub": "ana",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    }));
    assert!(party.verify(&authorized).is_ok());
}

/// A different issuer's key set is a different issuer, and the `iss` check is what says so even
/// when the signature is real.
#[test]
fn a_token_from_another_issuer_is_refused() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    let elsewhere = issuer.id_token_with(serde_json::json!({
        "iss": "https://login.evil.test", "aud": CLIENT, "sub": "ana",
        "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
    }));
    assert!(party.verify(&elsewhere).is_err());
}

/// Expiry against the injected clock, in both directions and with the skew allowance stated.
#[test]
fn expiry_is_read_from_the_clock_the_process_was_given() {
    let issuer = Issuer::new();
    let clock = at(NOW);
    let party = relying_party(&issuer, clock.clone());
    let token = issuer.id_token("ana");
    assert!(party.verify(&token).is_ok());

    // A second past the expiry is inside the skew allowance and still accepted; well past it is
    // not. Both are statements about an instant rather than about how long the test took.
    clock.set(NOW + 3_601_000);
    assert!(party.verify(&token).is_ok(), "one second of skew");
    clock.set(NOW + 3_600_000 + beck_rt::oidc::CLOCK_SKEW_MS + 1_000);
    assert!(party.verify(&token).is_err(), "a minute and a second");
}

/// A key set with one key and a token naming another `kid` is a rotation this process has not seen
/// — refused now, and a refetch scheduled rather than performed on the connection path.
#[test]
fn an_unknown_key_is_refused_and_schedules_a_refetch() {
    let issuer = Issuer::new();
    let clock = at(NOW);
    let party = relying_party(&issuer, clock.clone());
    assert!(!party.refresh_due(), "nothing is due right after a fetch");

    let stranger = issuer.token(
        serde_json::json!({"alg": "RS256", "kid": "a-key-nobody-published"}),
        serde_json::json!({
            "iss": ISSUER, "aud": CLIENT, "sub": "ana",
            "exp": (NOW / 1_000) + 3_600, "iat": NOW / 1_000,
        }),
    );
    assert!(party.verify(&stranger).is_err());

    // Still not due: the floor between fetches is what stops "verify this token" being a way to
    // make this process call its identity provider.
    assert!(
        !party.refresh_due(),
        "an unknown kid caused a fetch inside the floor"
    );
    clock.set(NOW + beck_rt::oidc::REFETCH_FLOOR_MS);
    assert!(
        party.refresh_due(),
        "and it is due once the floor has passed"
    );
}

/// The issuer must be `https`, and there is no flag that changes it: the key set's only integrity
/// protection is the transport it arrived over.
#[test]
fn a_plaintext_issuer_is_not_configurable() {
    let issuer = Issuer::new();
    let party = RelyingParty::new(
        Config::new("http://login.acme.test", CLIENT, REDIRECT),
        at(NOW),
        Arc::new(Network(issuer.clone())),
    );
    let why = party.refresh().expect_err("a plaintext issuer is refused");
    assert!(why.contains("https"), "{why}");
}

// ---------------------------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------------------------

/// The whole authorization-code exchange, end to end: what the browser is sent to, what comes
/// back, and the token that results.
#[test]
fn the_code_flow_completes_and_the_token_is_the_issuers() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));

    let begun = party.begin_login("/todos").expect("a login starts");
    assert!(
        begun.url.starts_with(&format!("{ISSUER}/authorize?")),
        "{}",
        begun.url
    );
    let asked = beck_rt::oidc::query_params(begun.url.split_once('?').expect("a query").1);
    let param = |name: &str| {
        asked
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no `{name}` in {}", begun.url))
    };
    assert_eq!(param("response_type"), "code");
    assert_eq!(param("client_id"), CLIENT);
    assert_eq!(param("redirect_uri"), REDIRECT);
    assert!(param("scope").split(' ').any(|s| s == "openid"));
    // PKCE, and the S256 method rather than `plain` — which is the only one worth having.
    assert_eq!(param("code_challenge_method"), "S256");
    assert!(!param("code_challenge").is_empty());

    // The issuer will answer the exchange with a token carrying the nonce it was asked for.
    issuer
        .code_to_nonce
        .lock()
        .expect("not poisoned")
        .push(("the-code".to_string(), param("nonce")));

    let done = party
        .complete_login(
            &format!("code=the-code&state={}", param("state")),
            &begun.transaction,
        )
        .expect("the exchange completes");
    assert_eq!(done.verified.subject, "ana");
    assert_eq!(
        done.return_to, "/todos",
        "the browser goes back where it was"
    );

    // What went to the token endpoint: the code, the verifier, and this client.
    let sent = issuer.exchanges.lock().expect("not poisoned");
    let form = beck_rt::oidc::query_params(&sent[0].body);
    let field = |name: &str| form.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    assert_eq!(field("grant_type").as_deref(), Some("authorization_code"));
    assert_eq!(field("code").as_deref(), Some("the-code"));
    assert_eq!(field("redirect_uri").as_deref(), Some(REDIRECT));
    assert!(field("code_verifier").is_some(), "PKCE's other half");

    // And the cookie the browser will carry is the issuer's token: verifying it again needs
    // nothing this process kept.
    let fresh = relying_party(&issuer, at(NOW));
    assert_eq!(
        fresh
            .verify(&done.id_token)
            .expect("it stands alone")
            .name(),
        "ana"
    );
}

/// The three ways a callback is not the login it claims to be.
#[test]
fn a_callback_that_is_not_the_login_we_started_is_refused() {
    let issuer = Issuer::new();
    let clock = at(NOW);
    let party = relying_party(&issuer, clock.clone());
    let begun = party.begin_login("/").expect("a login starts");
    let state = beck_rt::oidc::query_params(begun.url.split_once('?').expect("a query").1)
        .into_iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v)
        .expect("a state");

    // A state that is not the one we asked for — the CSRF the parameter exists for.
    assert!(party
        .complete_login("code=the-code&state=somebody-elses", &begun.transaction)
        .is_err());

    // A transaction cookie this process did not seal.
    assert!(party
        .complete_login(
            &format!("code=the-code&state={state}"),
            "state.nonce.verifier.99999999999999.Lw.0000"
        )
        .is_err());

    // The same login, ten minutes and a second later.
    clock.set(NOW + beck_rt::oidc::LOGIN_WINDOW_MS + 1_000);
    let why = party
        .complete_login(&format!("code=the-code&state={state}"), &begun.transaction)
        .expect_err("the window closed");
    assert!(why.contains("window"), "{why}");
}

/// A `next` that names a host is an open redirect, so it is not one.
///
/// Checked through the whole flow rather than against an internal function, because what matters
/// is the `Location` a browser is eventually given.
#[test]
fn a_return_target_may_only_be_a_path() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));
    for hostile in ["https://evil.test/", "//evil.test/", "javascript:alert(1)"] {
        let begun = party.begin_login(hostile).expect("a login starts");
        let asked = beck_rt::oidc::query_params(begun.url.split_once('?').expect("a query").1);
        let param = |name: &str| {
            asked
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .expect("a parameter")
        };
        let code = format!("code-for-{hostile}");
        issuer
            .code_to_nonce
            .lock()
            .expect("not poisoned")
            .push((code.clone(), param("nonce")));
        let done = party
            .complete_login(
                &format!("code={code}&state={}", param("state")),
                &begun.transaction,
            )
            .expect("the exchange completes");
        assert_eq!(
            done.return_to, "/",
            "`{hostile}` survived into the return target"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Through the application
// ---------------------------------------------------------------------------------------------

/// The point of all of it: a program's `Session` carries the claims the **issuer** made, and the
/// actor is the one the token names rather than the one the frame asks for.
#[tokio::test]
async fn the_claims_reach_the_program() {
    let issuer = Issuer::new();
    let party = Arc::new(relying_party(&issuer, at(NOW)));
    let app = beck_rt::App::start(
        support::todo_runtime(),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig {
            identity: party.clone(),
            ..Default::default()
        },
    )
    .await
    .expect("the app starts");

    let actor = party
        .verify(&issuer.id_token("ana"))
        .expect("the token verifies");
    let session = app.runtime().session(&actor);
    assert_eq!(
        session.field("actor").and_then(|v| v.as_str()),
        Some("ana"),
        "{session:?}"
    );
    let claims = session.field("claims").expect("a claims field");
    let email = claims
        .as_map()
        .expect("claims is a map")
        .get(&beck_core::Value::str_("email"))
        .cloned();
    assert_eq!(
        email.as_ref().and_then(|v| v.as_str()),
        Some("ana@acme.test"),
        "a verified claim did not reach the Session the program sees"
    );

    // And a name with no credential behind it has none — which is what makes reading one a check.
    let anonymous = app.runtime().session("ana");
    assert_eq!(
        anonymous
            .field("claims")
            .and_then(|v| v.as_map())
            .map(|m| m.len()),
        Some(0)
    );
}

/// What a verification costs, printed rather than thresholded.
///
/// It matters because of a design decision rather than a curiosity: the cookie is the **issuer's**
/// token, so this runs on every connection and on every document, not once at login. §13.7's rule
/// is that a shared runner cannot hold a wall-clock threshold honestly, so nothing here is asserted
/// — what is asserted is the **shape**, at two sizes, because one measurement cannot tell a fixed
/// cost from one that grows with the token.
///
///     cargo test --release -p beck-cli --test oidc -- --nocapture the_cost_of_a_verification
#[test]
fn the_cost_of_a_verification() {
    let issuer = Issuer::new();
    let party = relying_party(&issuer, at(NOW));

    // The same token with 5 and with 68 claims: the modular exponentiation is a fixed cost and
    // reading the token is not, so the second size says which of the two dominates.
    let sized = |total: usize| {
        let mut claims = serde_json::Map::new();
        claims.insert("iss".into(), ISSUER.into());
        claims.insert("aud".into(), CLIENT.into());
        claims.insert("sub".into(), "ana".into());
        claims.insert("exp".into(), ((NOW / 1_000) + 3_600).into());
        for i in 0..total.saturating_sub(claims.len()) {
            claims.insert(format!("claim_{i}"), format!("value-{i}").into());
        }
        assert_eq!(
            claims.len(),
            total,
            "the label has to be the number of claims"
        );
        issuer.id_token_with(serde_json::Value::Object(claims))
    };

    for claims in [5usize, 68] {
        let token = sized(claims);
        // Warm, then measure: the first call parses a key the later ones find in the same set.
        assert!(party.verify(&token).is_ok());
        let runs = 200;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            let _ = party.verify(&token);
        }
        let each = start.elapsed() / runs;
        println!(
            "verify: {claims:3} claims, {:5} bytes — {:?} each",
            token.len(),
            each
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The declaration
// ---------------------------------------------------------------------------------------------
//
// `identity = external(issuer="…")` is D6's block. What makes it a declaration rather than a flag
// is this section: the compiler knows the issuer, so §6.5's derivation does too.

/// A program with an identity declaration and nothing else about networking.
fn declaring(issuer: &str) -> String {
    format!(
        r#"
identity = external(issuer="{issuer}")

model State:
    n: Int

union Command:
    Bump

union Event:
    Bumped

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(n=s.n + 1)

def validate(s: State, p: Proposal) -> Result[list[Event], Str]:
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, book, validate)
book: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(book, view)
"#
    )
}

/// The whole reason the issuer is in the program: `beck build` can write the egress rule for it.
///
/// Both directions, because "the rule contains the host" would also pass against an emitter that
/// allowed everything: a program that declares nothing gets no rule for it.
#[test]
fn the_declared_issuer_is_the_egress_rule() {
    let src = declaring("https://login.acme.com");
    let (placed, diags, map) = beck_core::compile_str("declared.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("it slices");

    let beck_core::check::IdentityDecl::External { issuer, host, .. } = placed
        .program
        .identity
        .as_ref()
        .expect("the program declares an identity provider")
    else {
        panic!("`external(…)` did not read as external");
    };
    assert_eq!(issuer.as_ref(), "https://login.acme.com");
    assert_eq!(
        host.as_ref(),
        "login.acme.com",
        "the host is what an egress rule is written from"
    );

    let rendered = manifests(&placed);
    assert!(
        rendered.contains("login.acme.com"),
        "the derived policy cannot reach the issuer, so a deployed pod could not fetch its key set"
    );
    // The same program without the line: no rule, and therefore nothing to have got right by
    // accident.
    let bare = src.replace(
        "identity = external(issuer=\"https://login.acme.com\")\n",
        "",
    );
    let (placed, diags, map) = beck_core::compile_str("bare.beck", &bare);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let rendered = manifests(&placed.expect("it slices"));
    assert!(
        !rendered.contains("login.acme.com"),
        "a program that names no issuer got an egress rule for one:\n{rendered}"
    );
}

fn manifests(placed: &beck_core::Placed) -> String {
    beck_infra::k8s::render(&beck_infra::graph(placed), &placed.wire_id)
        .into_iter()
        .map(|(_, yaml)| yaml)
        .collect()
}

/// `identity = managed()` provisions the provider, wired to this application (D6).
///
/// The claim is the *wiring*, not that a pod starts — `beck-infra/tests/conformance.rs` skips
/// without a cluster and there is none here. So what is asserted is that the objects refer to each
/// other: the realm admits the redirect URI the route serves, the application is told the issuer
/// that names the Service, and the egress rule is a peer rather than a DNS name.
#[test]
fn managed_identity_provisions_a_provider_and_wires_it_to_this_application() {
    let src = declaring("https://login.acme.com").replace(
        "identity = external(issuer=\"https://login.acme.com\")",
        "identity = managed()",
    );
    let (placed, diags, map) = beck_core::compile_str("managed.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("it slices");
    assert!(matches!(
        placed.program.identity,
        Some(beck_core::check::IdentityDecl::Managed { .. })
    ));

    let graph = beck_infra::graph(&placed);
    let app = &graph.app;
    let provider = graph
        .nodes
        .iter()
        .find_map(|d| match &d.node {
            beck_infra::Node::IdentityProvider { name, realm, .. } => Some((name, realm)),
            _ => None,
        })
        .expect("a provider is provisioned");
    assert_eq!(provider.0, &format!("{app}-identity"));

    // The realm is derived from the two things the graph already knows: this application's name,
    // and the host its own Route admits. A realm whose redirect URI is not the path the runtime
    // serves is a provider that refuses every login with `invalid_redirect_uri`.
    let realm: serde_json::Value = serde_json::from_str(provider.1).expect("the realm is JSON");
    assert_eq!(realm["realm"], app.as_str());
    assert_eq!(
        realm["clients"][0]["redirectUris"][0],
        format!(
            "https://{}{}",
            beck_infra::route_host(app),
            beck_rt::http::CALLBACK_PATH
        ),
        "the realm admits a URI the application does not serve"
    );

    // The application's egress to it is a **peer**, which Kubernetes enforces — an external issuer
    // can only be an `allow_egress_hosts` entry, which it does not (§48.8).
    let policy = graph
        .nodes
        .iter()
        .find_map(|d| match &d.node {
            beck_infra::Node::Policy {
                allow_egress_to,
                allow_egress_hosts,
                ..
            } => Some((allow_egress_to, allow_egress_hosts)),
            _ => None,
        })
        .expect("a policy");
    assert!(
        policy.0.iter().any(|p| p.app == format!("{app}-identity")),
        "the application may not reach the provider this deployment stood up: {:?}",
        policy.0
    );
    assert!(
        policy.1.is_empty(),
        "a provisioned provider became a DNS-name rule, which the cluster cannot enforce: {:?}",
        policy.1
    );

    // And `beck explain deploy` says *why* the rule is there. A policy with an egress peer and a
    // provenance line reading "no `net.out` in the program, no egress rule in the cluster" is an
    // explanation that contradicts the object it explains.
    let why = graph.explain();
    assert!(
        why.contains("`identity = managed()`"),
        "the policy's egress peer has no explanation:\n{why}"
    );

    // And the same program without the declaration provisions nothing, so none of the above is
    // something the emitter does anyway.
    let (bare, _, _) =
        beck_core::compile_str("bare.beck", &src.replace("identity = managed()\n", ""));
    let bare = beck_infra::graph(&bare.expect("it slices"));
    assert!(
        !bare
            .nodes
            .iter()
            .any(|d| matches!(d.node, beck_infra::Node::IdentityProvider { .. })),
        "a program that asked for nothing got a provider"
    );
}

/// The container is told where its issuer is, and who it is there — so an operator writes neither.
///
/// "Wired via OIDC automatically" is D6's phrase, and this is what it has to mean: the client id is
/// the realm's, the redirect URI is the route's, and the issuer is the Service's. Read off the
/// rendered manifest rather than off the functions that produced it, which is `docs/92` §92.2's
/// rule — calling them here would be agreeing with myself.
#[test]
fn the_deployment_tells_the_application_where_its_issuer_is() {
    let src = declaring("https://login.acme.com").replace(
        "identity = external(issuer=\"https://login.acme.com\")",
        "identity = managed()",
    );
    let (placed, diags, map) = beck_core::compile_str("managed.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("it slices");
    let rendered = manifests(&placed);

    // The env var the runtime reads, from the Secret the graph derived.
    assert!(
        rendered.contains(beck_infra::provider::DEFAULT.issuer_var),
        "the application container is not told its issuer:\n{rendered}"
    );
    // The issuer value itself: the Service, the provider's port and the realm.
    assert!(
        rendered.contains(&beck_infra::provider::DEFAULT.issuer(
            &format!("{}-identity", placed.program.name.replace('_', "-")),
            &placed.program.name.replace('_', "-")
        )),
        "the issuer written into the Secret does not name the Service the graph emitted:\n{rendered}"
    );
}

/// Every way the declaration can be wrong, refused with a span.
///
/// The two that matter are the two `beck_rt::oidc` also refuses at run time, and refusing them here
/// is the difference between a diagnostic and a deployment that builds and authenticates nobody.
#[test]
fn a_declaration_this_compiler_cannot_deploy_is_a_diagnostic() {
    let refused = |what: &str, src: String| {
        let (_, diags, map) = beck_core::compile_str("bad.beck", &src);
        assert!(
            diags.iter().any(|d| d.code == "B0359"),
            "{what} was accepted:\n{}",
            diags.render(&map)
        );
    };
    // Plaintext: the key set has no integrity protection but the transport it arrives over.
    refused("a plaintext issuer", declaring("http://login.acme.com"));
    // A host §6.5 could not write a rule from.
    refused(
        "an issuer with credentials",
        declaring("https://a:b@login.acme.com"),
    );
    refused("an issuer that is not a URL", declaring("login.acme.com"));
    // A provider that is neither of D6's two.
    refused(
        "an unknown provider",
        declaring("https://login.acme.com").replace(
            "external(issuer=\"https://login.acme.com\")",
            "whatever(issuer=\"https://login.acme.com\")",
        ),
    );
    // `managed()` provisions the provider, so there is nothing to tell it about: an issuer here
    // would be a URL the deployment is about to decide for itself.
    refused(
        "managed with an issuer",
        declaring("https://login.acme.com").replace(
            "external(issuer=\"https://login.acme.com\")",
            "managed(issuer=\"https://login.acme.com\")",
        ),
    );
    // And two declarations are two answers to one question.
    refused(
        "a second declaration",
        declaring("https://login.acme.com").replace(
            "identity = external(issuer=\"https://login.acme.com\")",
            "identity = external(issuer=\"https://login.acme.com\")\nidentity = external(issuer=\"https://login.other.com\")",
        ),
    );
}

/// `identity` is an ordinary name, and a program that uses it as one still compiles.
///
/// `sicp/ch1.beck` defines `identity` at §1.3.1, and the declaration is guarded on the `=` rather than on the
/// word for exactly that reason. This is the gate that says so.
#[test]
fn identity_is_still_a_name_a_program_may_use() {
    let src = "def identity(n: Int) -> Int:\n    return n\n\ndef twice(n: Int) -> Int:\n    return identity(n) + identity(n)\n";
    let (_, diags, map) = beck_core::check_str("names.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
}

// ---------------------------------------------------------------------------------------------
// The HTTP edge, over a real socket
// ---------------------------------------------------------------------------------------------
//
// `runtime_edge.rs`'s argument, applied to a second edge: a refusal wired into a handler and tested
// only as a pure function is a refusal one refactor away from never being called. So the client
// below is a `TcpStream` and a handful of literal bytes, which is what a browser sends.

async fn serve_with(party: Arc<RelyingParty>) -> std::net::SocketAddr {
    let app = beck_rt::App::start(
        support::todo_runtime(),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig {
            identity: party,
            ..Default::default()
        },
    )
    .await
    .expect("the app starts");
    let listener = beck_rt::http::bind("127.0.0.1:0".parse().expect("an address"))
        .await
        .expect("an ephemeral port");
    let addr = listener.local_addr().expect("a bound address");
    drop(listener);
    let (_tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = beck_rt::http::serve(app, addr, rx).await;
    });
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    std::mem::forget(_tx); // the server lives as long as the test
    addr
}

/// One request, as bytes, and the whole response as text.
async fn request(addr: std::net::SocketAddr, line: &str, headers: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("the server is up");
    stream
        .write_all(format!("{line} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}\r\n").as_bytes())
        .await
        .expect("the request is written");
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("the server answers in time")
        .expect("the server answers");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The three things a browser sees: no credential sends you to the issuer, a bad one is refused
/// before anything is rendered, and a good one in a cookie gets the page.
#[tokio::test]
async fn the_edge_sends_a_visitor_to_the_issuer_and_refuses_a_bad_credential() {
    let issuer = Issuer::new();
    let party = Arc::new(relying_party(&issuer, at(NOW)));
    let addr = serve_with(party).await;

    // No cookie: a person who is not signed in has somewhere to go.
    let answer = request(addr, "GET /", "").await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");
    assert!(answer.contains("location: /auth/login"), "{answer}");

    // `/auth/login` sends the browser to the issuer's authorization endpoint, and seals what it
    // asked for into a cookie a script cannot read.
    let begun = request(addr, "GET /auth/login?next=/todos", "").await;
    assert!(begun.starts_with("HTTP/1.1 302"), "{begun}");
    assert!(
        begun.contains(&format!("location: {ISSUER}/authorize?")),
        "{begun}"
    );
    assert!(begun.contains("beck_login="), "{begun}");
    assert!(
        begun.contains("HttpOnly") && begun.contains("SameSite=Lax"),
        "the login cookie is readable by a script: {begun}"
    );

    // A cookie that is not a token this issuer signed does not get a page.
    let forged = request(addr, "GET /", "Cookie: beck_id=not.a.token\r\n").await;
    assert!(forged.starts_with("HTTP/1.1 302"), "{forged}");
    assert!(
        forged.contains("beck_id=;"),
        "a rejected credential was left in place: {forged}"
    );

    // And a real one does — with the actor the *token* names in the document.
    let good = request(
        addr,
        "GET /",
        &format!("Cookie: beck_id={}\r\n", issuer.id_token("ana")),
    )
    .await;
    assert!(good.starts_with("HTTP/1.1 200"), "{good}");
    assert!(good.contains("data-b-actor=\"ana\""), "{good}");

    // And the claims the token carried, because a Mode B client runs the program's own `validate`
    // against them: a document with the actor but not the claims is a browser that would refuse
    // what the server accepts (`mode_b.rs`). Escaped for the attribute it sits in, so the assertion
    // is written the way the browser will read it.
    assert!(
        good.contains("data-b-claims=\"{&quot;email&quot;:&quot;ana@acme.test&quot;"),
        "the document did not carry the claims the token was verified to hold: {good}"
    );
    // The protocol's own fields are not claims about the person and do not travel (`PROTOCOL_CLAIMS`).
    assert!(
        !good.contains("&quot;iss&quot;") && !good.contains("&quot;exp&quot;"),
        "a protocol field reached the browser as a claim: {good}"
    );
}

/// A claim is the issuer's string, and an issuer that puts a quote in one must not be able to
/// write an attribute of its own into the document it lands in.
#[tokio::test]
async fn a_claim_cannot_break_out_of_the_attribute_it_is_written_into() {
    let issuer = Issuer::new();
    let party = Arc::new(relying_party(&issuer, at(NOW)));
    let addr = serve_with(party).await;

    let token = issuer.id_token_with(serde_json::json!({
        "iss": ISSUER,
        "aud": CLIENT,
        "sub": "ana",
        "exp": (NOW / 1_000) + 3_600,
        "iat": NOW / 1_000,
        "tenant": "\"><script>alert(1)</script>",
    }));
    let page = request(addr, "GET /", &format!("Cookie: beck_id={token}\r\n")).await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(
        !page.contains("<script>alert(1)</script>"),
        "a claim closed its attribute and wrote markup: {page}"
    );
    assert!(page.contains("&lt;script&gt;"), "{page}");
}

/// The socket is where the property that matters lives: the credential is in a **cookie**, so the
/// `hello` frame cannot carry it and cannot be used to claim somebody else.
#[tokio::test]
async fn the_websocket_upgrade_is_where_a_cookie_is_checked() {
    let issuer = Issuer::new();
    let party = Arc::new(relying_party(&issuer, at(NOW)));
    let addr = serve_with(party).await;
    let handshake = "Upgrade: websocket\r\nConnection: Upgrade\r\n\
                     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n";

    let refused = request(
        addr,
        "GET /socket",
        &format!("{handshake}Cookie: beck_id=not.a.token\r\n"),
    )
    .await;
    assert!(
        refused.starts_with("HTTP/1.1 401"),
        "a forged cookie opened a socket: {refused}"
    );

    let accepted = request(
        addr,
        "GET /socket",
        &format!("{handshake}Cookie: beck_id={}\r\n", issuer.id_token("ana")),
    )
    .await;
    assert!(
        accepted.starts_with("HTTP/1.1 101"),
        "a verified cookie did not open a socket: {accepted}"
    );
}
