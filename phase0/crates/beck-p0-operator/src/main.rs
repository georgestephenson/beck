//! `beck-p0-operator` — the infrastructure tier.
//!
//! Two jobs, matching §6.3 and §6.4:
//!
//! * `emit` builds the `InfraGraph` from the program's effects and writes it out — typed objects
//!   in, YAML out, so GitOps users get files and `beck deploy` gets server-side apply.
//! * `run` is the operator: it reconciles `BeckApplication` and owns *ordering and provenance* —
//!   the deploy-rides-the-stream choreography. Phase 0 ships the control loop and the decision
//!   function; the choreography's individual steps are Phase 4.

mod controller;
mod crd;
mod infra;
mod yaml;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use kube::CustomResourceExt;

use crate::infra::{InfraGraph, Substrate};

#[derive(Parser)]
#[command(name = "beck-p0-operator", about = "Phase 0 infrastructure tier")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Emit the object graph derived from the program's effects.
    Emit {
        #[arg(long, default_value = "deploy/k8s")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = SubstrateArg::Postgres)]
        substrate: SubstrateArg,
        /// Fail instead of writing if the emitted graph differs from what is on disk.
        #[arg(long)]
        check: bool,
    },
    /// Print the `BeckApplication` CustomResourceDefinition.
    Crd,
    /// Run the reconciler. Needs a cluster.
    Run,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum SubstrateArg {
    Postgres,
    Embedded,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match Cli::parse().command {
        Cmd::Emit {
            out,
            substrate,
            check,
        } => emit(out, substrate, check),
        Cmd::Crd => {
            print!(
                "---\n{}",
                yaml::to_yaml(&serde_json::to_value(crd::BeckApplication::crd())?)
            );
            Ok(())
        }
        Cmd::Run => controller::run().await,
    }
}

fn emit(out: PathBuf, substrate: SubstrateArg, check: bool) -> Result<()> {
    let graph = InfraGraph::todo_app(match substrate {
        SubstrateArg::Postgres => Substrate::Postgres,
        SubstrateArg::Embedded => Substrate::Embedded,
    });

    let mut files = graph.files();
    files.push((
        "80-crd.yaml".to_string(),
        yaml::documents(&[serde_json::to_value(crd::BeckApplication::crd())?]),
    ));
    files.push(("90-operator.yaml".to_string(), controller::rbac()));

    let mut differences = Vec::new();
    std::fs::create_dir_all(&out)?;
    for (name, body) in files {
        if body.trim().is_empty() {
            continue;
        }
        let path = out.join(&name);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing == body {
            continue;
        }
        if check {
            differences.push(name);
        } else {
            std::fs::write(&path, &body)?;
            println!("wrote {}", path.display());
        }
    }

    if !differences.is_empty() {
        anyhow::bail!(
            "generated manifests are out of date: {}\nrun `beck-p0-operator emit`",
            differences.join(", ")
        );
    }
    if check {
        println!("manifests are up to date");
    }
    Ok(())
}
