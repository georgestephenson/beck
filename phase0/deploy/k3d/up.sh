#!/usr/bin/env bash
# Rung 3 of the parity ladder (§6.6): a local k3s cluster, real manifests, real operator.
#
# Rungs 0 and 3 are the two that must be excellent. Rung 0 (`beck-p0 run`) needs nothing but this
# repository; this script is rung 3, and it needs a container runtime — which is exactly why the
# language must never require one for rung 0.
#
# Phase 0 note: this script has not been executed. The environment the Phase 0 work was done in
# has no container daemon, so the manifests it applies are validated (they parse, they carry
# apiVersion/kind, and they are generated from typed objects) but were never reconciled by a real
# API server. Running it is the first task of Phase 1.
#
#   ./deploy/k3d/up.sh          # create the cluster, build, load and apply
#   ./deploy/k3d/up.sh down     # delete it

set -euo pipefail

CLUSTER="${CLUSTER:-beck-p0}"
IMAGE="${IMAGE:-beck/phase0-todo:dev}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

need() { command -v "$1" >/dev/null || { echo "missing: $1"; exit 1; }; }

if [[ "${1:-up}" == "down" ]]; then
  need k3d
  k3d cluster delete "$CLUSTER"
  exit 0
fi

need k3d
need kubectl
need apko
need cargo

echo "==> building the service binary (static, so the image needs no libc)"
cargo build --release --target x86_64-unknown-linux-musl -p beck-p0-server --manifest-path "$ROOT/Cargo.toml"
cp "$ROOT/target/x86_64-unknown-linux-musl/release/beck-p0" "$ROOT/deploy/apko/beck-p0"

echo "==> building the image with apko (daemonless, reproducible)"
apko build "$ROOT/deploy/apko/beck-p0.yaml" "$IMAGE" "$ROOT/deploy/apko/beck-p0.tar"

echo "==> creating the cluster"
k3d cluster create "$CLUSTER" \
  --agents 2 \
  --port "8080:80@loadbalancer" \
  --k3s-arg "--disable=traefik@server:0"

echo "==> installing the Gateway API CRDs and a gateway"
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/standard-install.yaml
kubectl create namespace gateway-system --dry-run=client -o yaml | kubectl apply -f -
kubectl label namespace gateway-system kubernetes.io/metadata.name=gateway-system --overwrite

echo "==> loading the image"
k3d image import "$ROOT/deploy/apko/beck-p0.tar" --cluster "$CLUSTER"

echo "==> applying the generated object graph"
# Regenerate first, so what is applied is what the effects imply — not what someone edited.
cargo run --release -p beck-p0-operator --manifest-path "$ROOT/Cargo.toml" -- emit --out "$ROOT/deploy/k8s"
kubectl apply -f "$ROOT/deploy/k8s/80-crd.yaml"
kubectl apply -f "$ROOT/deploy/k8s/00-namespace.yaml"
kubectl label namespace beck-todo kubernetes.io/metadata.name=beck-todo --overwrite
kubectl create secret generic beck-postgres \
  --namespace beck-todo \
  --from-literal=password=beck \
  --from-literal=url='postgres://postgres:beck@beck-postgres.beck-todo.svc:5432/beck' \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f "$ROOT/deploy/k8s/"

echo "==> waiting for the log store, then the application"
kubectl -n beck-todo rollout status statefulset/beck-postgres --timeout=180s
kubectl -n beck-todo exec statefulset/beck-postgres -- \
  psql -U postgres -d beck -f - < "$ROOT/deploy/postgres/grants.sql" || \
  echo "note: grants apply after the DDL exists; rerun after the app's first start"
kubectl -n beck-todo rollout status deployment/beck-todo --timeout=180s

cat <<EOF

the todo app is running in a local cluster.

  open      http://todo.beck.localhost:8080   (add to /etc/hosts, or curl -H 'Host: todo.beck.localhost')
  status    kubectl -n beck-todo get beckapplication beck-todo -o yaml
  logs      kubectl -n beck-todo logs deploy/beck-todo -f
  replay    kubectl -n beck-todo exec deploy/beck-todo -- beck-p0 replay --store postgres --genesis
  teardown  ./deploy/k3d/up.sh down
EOF
