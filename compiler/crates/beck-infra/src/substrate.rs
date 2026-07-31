//! What a `durable` fold is actually stored in, named once.
//!
//! # Why this module exists
//!
//! The `durable` effect implies a [`crate::Node::LogStore`], and [`crate::Node`] is deliberately
//! free of vendor nouns — it says "a log store with a volume", not "Postgres". Every platform then
//! has to turn that into something concrete, and before this module existed each of them turned it
//! into `postgres:16-alpine` on its own, with its own copy of the data directory, the port, the URL
//! format and the `--store` flag.
//!
//! That is five facts duplicated per platform, and it is exactly the shape that produced
//! docs/20 §20.4 item 13's third defect — two objects that had to agree, written twice, disagreeing.
//! With two platforms it would have been ten copies. So the facts live here, and a platform reads
//! them.
//!
//! # And it makes the substrate a decision rather than an assumption
//!
//! Postgres was chosen offhand in the original sketch and has never been revisited against what
//! Beck actually asks of a store: an append-only log with monotonic sequence numbers, batched
//! atomic appends, range reads by `seq`, and a snapshot blob. That is a much narrower contract than
//! "a database", and [`beck_rt::LogStore`](../../beck_rt/trait.LogStore.html) already expresses it
//! in seven methods with three implementations behind it — memory, redb and Postgres.
//!
//! What was missing was the *other* end: the runtime could speak to any store, and the deployment
//! could only provision one, because the knowledge was scattered. A second [`Substrate`] is now a
//! value, and the platforms pick it up without changing.
//!
//! Nothing here evaluates the choice — [`docs/07-dependencies.md`](../../../../docs/07-dependencies.md)
//! is where a substrate has to argue for itself. This module is what makes the argument actionable.

/// A concrete store behind the `durable` effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Substrate {
    /// What `beck run --store <name>` calls it. Must match the runtime's `Store` enum, because a
    /// deployment that provisions one store and starts the binary with another is a pod that comes
    /// up and cannot find its log.
    pub store: &'static str,
    /// The container image that provides it.
    pub image: &'static str,
    /// The port it listens on.
    pub port: u16,
    /// Where its data lives inside the container — the path a volume must be mounted at.
    pub data_dir: &'static str,
    /// The subdirectory of `data_dir` the process actually initialises, when it insists on an empty
    /// mount point. Postgres does; not every store will.
    pub data_subdir: Option<&'static str>,
    /// The environment variable the application reads its connection string from.
    pub url_var: &'static str,
}

/// The default, and the one thing all three tiers already speak.
///
/// The development password is `beck` and it is in the generated manifests on purpose: §6.6's
/// parity ladder wants rung 3 to work from `git clone` the way rung 0 does. A production deploy
/// overwrites the Secret.
pub const POSTGRES: Substrate = Substrate {
    store: "postgres",
    image: "postgres:16-alpine",
    port: crate::LOG_PORT,
    data_dir: "/var/lib/postgresql/data",
    data_subdir: Some("pgdata"),
    url_var: "BECK_POSTGRES_URL",
};

/// The substrate the emitter provisions.
///
/// One constant rather than a field on [`crate::Node::LogStore`], because today there is one and
/// pretending otherwise would be a configuration surface with nothing behind it. When there are
/// two, this becomes the default and the node carries the choice — and the platforms do not change,
/// which is the property this module is for.
pub const DEFAULT: Substrate = POSTGRES;

impl Substrate {
    /// The connection string that reaches this store at `host`.
    ///
    /// The one string two platforms must agree about: the host is a Kubernetes `Service` name in
    /// one and a Compose service name in the other, and everything around it is the same.
    pub fn url(&self, host: &str) -> String {
        format!("postgres://postgres:beck@{host}:{}/postgres", self.port)
    }

    /// The password the development default uses.
    pub fn dev_password(&self) -> &'static str {
        "beck"
    }

    /// Where the process is told to put its files, which is not always where the volume is mounted.
    pub fn pgdata(&self) -> String {
        match self.data_subdir {
            Some(sub) => format!("{}/{sub}", self.data_dir),
            None => self.data_dir.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_subdirectory_is_under_the_mount_point() {
        // Postgres refuses to initialise into a non-empty directory, and a volume mount is not
        // empty — so `PGDATA` is a subdirectory of the mount. Getting this backwards is a
        // StatefulSet that crash-loops on first start with a message about permissions.
        let s = POSTGRES;
        assert!(s.pgdata().starts_with(s.data_dir));
        assert_ne!(s.pgdata(), s.data_dir);
    }

    #[test]
    fn the_url_names_the_host_it_is_given_and_the_port_it_declares() {
        let url = POSTGRES.url("app-log.app.svc");
        assert!(url.contains("@app-log.app.svc:"), "{url}");
        assert!(url.contains(&POSTGRES.port.to_string()), "{url}");
    }
}
