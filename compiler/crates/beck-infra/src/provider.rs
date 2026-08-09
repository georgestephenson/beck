//! What `identity = managed()` is actually provisioned as, named once.
//!
//! [`crate::substrate`]'s argument, applied to the other thing a deployment stands up on a
//! program's behalf. [`crate::Node`] is free of vendor nouns — it says "an identity provider with a
//! volume", not "Keycloak" — and every platform has to turn that into something concrete. Written
//! per platform, that is an image, a port, a realm path, two environment variables and a mount
//! point duplicated per platform, which is the shape that produced `docs/20` §20.4 item 13's
//! defect.
//!
//! # Why Keycloak
//!
//! [`docs/10`](../../../../../docs/10-decisions.md) D6 chose it and gave the reason: "Passkeys, MFA,
//! social login are the IdP's features, inherited, not ours." Beck never stores a password and
//! never invents an auth protocol, so what a managed provider has to be is something that already
//! implements the protocol Beck is a relying party *to*. Apache-2.0, CNCF, and it speaks OIDC
//! discovery, which is the only interface [`beck_rt::oidc`](../../beck_rt/oidc/index.html) uses.
//!
//! # What it is *not*
//!
//! Not a claim that the pod starts. `beck-infra/tests/conformance.rs` skips without a cluster and
//! there is none here, so this module and its emitter establish "the object graph contains these
//! objects, wired to each other" — which is a different and smaller claim than "you can log in",
//! exactly as [`docs/82`](../../../../../docs/82-the-defaults-that-should-be-unavoidable-report.md)
//! §82.4 says of the pod defaults.

/// A concrete identity provider behind `identity = managed()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    /// The container image that provides it.
    pub image: &'static str,
    /// The port it listens on.
    pub port: u16,
    /// Where its data lives inside the container — the path a volume must be mounted at.
    pub data_dir: &'static str,
    /// Where a realm this deployment derived is dropped for the provider to import at startup.
    pub import_dir: &'static str,
    /// The argument that makes it read [`Provider::import_dir`].
    pub start_args: &'static [&'static str],
    /// The environment variable the application reads its issuer URL from.
    ///
    /// The same shape as [`crate::substrate::Substrate::url_var`], and for the same reason: the
    /// program says *that* it has a provider and the deployment says *where*, because a URL a
    /// program wrote would be one it has to be edited to move.
    pub issuer_var: &'static str,
}

/// The default, and D6's choice.
///
/// The development admin password is `beck` and it is in the generated manifests on purpose, which
/// is [`crate::substrate::POSTGRES`]'s decision and its reason: §6.6's parity ladder wants rung 3 to
/// work from `git clone`. A production deploy overwrites the Secret.
pub const KEYCLOAK: Provider = Provider {
    image: "quay.io/keycloak/keycloak:26.0",
    port: 8080,
    data_dir: "/opt/keycloak/data",
    import_dir: "/opt/keycloak/data/import",
    // `start-dev` rather than `start`: the production mode requires a hostname, a database and TLS
    // material this derivation does not have, and starting it in a mode it cannot satisfy is a
    // crash-loop rather than a deployment. §95.10 records that as the limit it is.
    start_args: &["start-dev", "--import-realm"],
    issuer_var: "BECK_IDENTITY_ISSUER",
};

/// The provider the emitter provisions.
///
/// One constant rather than a field on [`crate::Node::IdentityProvider`], for
/// [`crate::substrate::DEFAULT`]'s reason: today there is one, and a configuration surface with
/// nothing behind it is worse than none.
pub const DEFAULT: Provider = KEYCLOAK;

impl Provider {
    /// The issuer URL that reaches this provider at `host`, for realm `realm`.
    ///
    /// **`http`, and this is the one place in the project where that is not a defect.** An external
    /// issuer must be `https` because the key set has no integrity protection but the transport
    /// ([`docs/95`](../../../../../docs/95-oidc-relying-party-report.md) §95.2). A managed one is a
    /// `Service` this derivation emitted, in this application's own namespace, reachable only
    /// through a NetworkPolicy this derivation wrote — so what protects the key set is the policy,
    /// and §6.5's gateway is where TLS is terminated for everything else. §95.10 is where that
    /// argument is written down rather than left in a URL.
    pub fn issuer(&self, host: &str, realm: &str) -> String {
        format!("http://{host}:{}/realms/{realm}", self.port)
    }

    /// The admin password the development default uses.
    pub fn dev_password(&self) -> &'static str {
        "beck"
    }

    /// A realm with one public client, derived: the realm is the application, the client is the
    /// application, and the redirect URI is the route the same graph derived.
    ///
    /// A **public** client, because that is what a browser-facing application with PKCE is
    /// ([`docs/95`](../../../../../docs/95-oidc-relying-party-report.md) §95.3) — a confidential
    /// client would need a secret, and a secret this file invented and wrote into a manifest would
    /// be a secret in a git repository.
    pub fn realm(&self, app: &str, origin: &str) -> serde_json::Value {
        serde_json::json!({
            "realm": app,
            "enabled": true,
            "clients": [{
                "clientId": app,
                "enabled": true,
                "publicClient": true,
                "standardFlowEnabled": true,
                // Exactly the one path the relying party is served at, rather than a wildcard: an
                // authorization code is delivered to a redirect URI, so a loose one is somewhere
                // else a code can be delivered to.
                "redirectUris": [format!("{origin}/auth/callback")],
                "webOrigins": [origin.to_string()],
                "attributes": {"post.logout.redirect.uris": format!("{origin}/")},
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_directory_is_under_the_volume() {
        // The realm is mounted where the provider looks for it, and the volume covers that path —
        // getting this backwards is a pod that starts with an empty realm and refuses every login.
        assert!(KEYCLOAK.import_dir.starts_with(KEYCLOAK.data_dir));
        assert!(KEYCLOAK.start_args.contains(&"--import-realm"));
    }

    #[test]
    fn the_issuer_names_the_host_it_is_given_and_the_port_it_declares() {
        let url = KEYCLOAK.issuer("todo-identity", "todo");
        assert_eq!(url, "http://todo-identity:8080/realms/todo");
    }

    #[test]
    fn the_realm_admits_exactly_the_callback_this_runtime_serves() {
        let realm = KEYCLOAK.realm("todo", "https://todo.example.com");
        assert_eq!(realm["realm"], "todo");
        let client = &realm["clients"][0];
        assert_eq!(client["clientId"], "todo");
        assert_eq!(client["publicClient"], true);
        assert_eq!(
            client["redirectUris"][0], "https://todo.example.com/auth/callback",
            "the redirect must be the path `beck_rt::http` actually serves"
        );
        // A wildcard here is an open redirect the provider itself would honour.
        assert!(!client["redirectUris"][0]
            .as_str()
            .expect("a string")
            .contains('*'));
    }
}
