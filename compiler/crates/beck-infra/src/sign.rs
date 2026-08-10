//! Signing, in the shape `cosign` verifies.
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md) §6.2:
//! "**sign with Sigstore/cosign** and attach the SBOM and a provenance attestation".
//! [`docs/28`](../../../../../docs/28-releases-and-deployment.md) §28.2 puts "a cosign signature …
//! per artefact" in the release artefact set. This is the signature, produced by `beck` rather than
//! by a second tool, and it is deliberately the *keyed* half of Sigstore rather than the keyless
//! half — see below.
//!
//! # What a signature here actually says
//!
//! The payload is Sigstore's "simple signing" document: a reference, the **manifest digest**, and a
//! type string. Signing it says "the holder of this key asserts that this digest is that image".
//! Nothing more — in particular it says nothing about *how* the image was built, which is what a
//! SLSA provenance attestation says and which needs a builder identity this repository does not
//! have ([`docs/92`](../../../../../docs/92-sbom-report.md) §92.5, and unchanged).
//!
//! # Why keyed, and not keyless
//!
//! Keyless signing means Fulcio issuing a short-lived certificate against an OIDC identity and
//! Rekor logging the result — two network services and a workload identity. Neither exists for this
//! project ([`docs/28`](../../../../../docs/28-releases-and-deployment.md) §28.1: no release has
//! been cut), and a signing path that can only be exercised by a pipeline nobody has run is
//! [`docs/19`](../../../../../docs/19-phase-1-report.md) §19.4 item 10's design document again. A
//! keyed signature can be produced, verified and *gated* on a laptop, so it is the half that can be
//! built honestly today; the transparency log is what turns it into SLSA's build track, and that is
//! named as absent rather than approximated.
//!
//! # The one thing this module refuses to be clever about
//!
//! The payload's JSON is built as text, in field order, rather than through a serialiser. Sigstore
//! verifiers compare the payload bytes they were given against the bytes the signature covers, so
//! the document is a *byte string* and not a value — a serialiser that reordered two keys between
//! versions would produce a signature that verifies here and nowhere else.

use anyhow::{bail, Context, Result};
use aws_lc_rs::signature::{self, KeyPair as _};
use serde_json::{json, Value};

/// The media type of the payload cosign signs, and stores as the signature artefact's layer.
pub const PAYLOAD_MEDIA_TYPE: &str = "application/vnd.dev.cosign.simplesigning.v1+json";

/// The annotation the base64 signature is carried in, alongside the payload layer.
pub const SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";

/// The `critical.type` a container image signature carries. cosign refuses any other value.
pub const SIGNATURE_TYPE: &str = "cosign container image signature";

/// A P-256 signing key.
pub struct Key {
    pair: signature::EcdsaKeyPair,
    pkcs8: Vec<u8>,
}

impl Key {
    /// A fresh key.
    pub fn generate() -> Result<Key> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(
            &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &rng,
        )
        .map_err(|_| anyhow::anyhow!("the system random source would not produce a key"))?;
        Key::from_pkcs8(pkcs8.as_ref())
    }

    /// A key from its PKCS#8 DER encoding.
    pub fn from_pkcs8(der: &[u8]) -> Result<Key> {
        let pair =
            signature::EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, der)
                .map_err(|_| anyhow::anyhow!("not a P-256 private key in PKCS#8 form"))?;
        Ok(Key {
            pair,
            pkcs8: der.to_vec(),
        })
    }

    /// A key from the PEM a previous `beck key generate` wrote.
    pub fn from_pem(pem: &str) -> Result<Key> {
        Key::from_pkcs8(&pem_body(pem, "PRIVATE KEY")?)
    }

    /// The private key, PEM-encoded — PKCS#8, which is what an OpenSSL or a Go verifier reads.
    ///
    /// Not cosign's own `ENCRYPTED COSIGN PRIVATE KEY` container, which is a password-derived
    /// secretbox around this same DER. Writing one would mean asking for a password on a
    /// non-interactive path and implementing scrypt to no benefit: what cosign has to be able to
    /// read is the **public** key, and that is standard.
    pub fn private_pem(&self) -> String {
        pem("PRIVATE KEY", &self.pkcs8)
    }

    /// The public key, PEM-encoded — exactly what `cosign verify --key` takes.
    pub fn public_pem(&self) -> String {
        pem("PUBLIC KEY", &self.public_spki())
    }

    /// The public key as a SubjectPublicKeyInfo.
    ///
    /// The library hands back a raw uncompressed point, and a P-256 SPKI is that point behind a
    /// fixed 26-byte prefix — two algorithm identifiers and a bit-string header, none of which
    /// varies for this curve. Written out rather than assembled by a DER encoder, because a
    /// constant that a test compares against a known-good key is checkable and an encoder is one
    /// more thing to be right.
    pub fn public_spki(&self) -> Vec<u8> {
        const P256_SPKI_PREFIX: [u8; 26] = [
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        let mut out = P256_SPKI_PREFIX.to_vec();
        out.extend_from_slice(self.pair.public_key().as_ref());
        out
    }

    /// Sign a payload, returning the base64 signature cosign stores in an annotation.
    pub fn sign(&self, payload: &[u8]) -> Result<String> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let sig = self
            .pair
            .sign(&rng, payload)
            .map_err(|_| anyhow::anyhow!("signing failed"))?;
        Ok(base64(sig.as_ref()))
    }
}

/// Check a signature against a public key in the PEM form [`Key::public_pem`] writes.
///
/// Verification goes through the public key alone — no access to the pair that made the signature —
/// because a verifier that could reach the private key would be checking that this module agrees
/// with itself.
pub fn verify(payload: &[u8], signature_b64: &str, public_pem: &str) -> Result<()> {
    let spki = pem_body(public_pem, "PUBLIC KEY")?;
    // The point is the tail of the SubjectPublicKeyInfo; `UnparsedPublicKey` wants it bare.
    let point = spki
        .get(26..)
        .filter(|p| p.len() == 65 && p[0] == 0x04)
        .context("not a P-256 public key")?;
    let sig = unbase64(signature_b64).context("the signature is not base64")?;
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, point)
        .verify(payload, &sig)
        .map_err(|_| anyhow::anyhow!("the signature does not verify against this key"))
}

/// The document cosign signs for an image.
///
/// Built as text, in cosign's field order — see this module's opening for why this is not a
/// `serde_json` value.
pub fn payload(reference: &str, manifest_digest: &str) -> String {
    format!(
        "{{\"critical\":{{\"identity\":{{\"docker-reference\":{reference}}},\
         \"image\":{{\"docker-manifest-digest\":{digest}}},\
         \"type\":{ty}}},\"optional\":null}}",
        reference = json!(reference),
        digest = json!(manifest_digest),
        ty = json!(SIGNATURE_TYPE),
    )
}

/// A signature, as the artefact that travels beside the image.
pub struct Signature {
    /// The payload bytes the signature covers.
    pub payload: Vec<u8>,
    /// The base64 signature.
    pub signature: String,
    /// The tag cosign stores it under: `sha256-<hex>.sig`.
    pub tag: String,
    /// The manifest of the signature artefact.
    pub manifest: Vec<u8>,
    /// The empty config blob the manifest points at.
    pub config: Vec<u8>,
}

/// The empty JSON object every cosign signature manifest uses as its config blob.
const EMPTY_CONFIG: &[u8] = b"{}";

/// Sign an image manifest digest, producing the artefact cosign expects to find beside it.
pub fn image(key: &Key, reference: &str, manifest_digest: &str) -> Result<Signature> {
    if !manifest_digest.starts_with("sha256:") {
        bail!("{manifest_digest} is not a sha256 digest");
    }
    let payload = payload(reference, manifest_digest).into_bytes();
    let signature = key.sign(&payload)?;

    let mut layer = descriptor(PAYLOAD_MEDIA_TYPE, &payload);
    layer["annotations"] = json!({ SIGNATURE_ANNOTATION: signature });
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": crate::oci::MANIFEST_MEDIA_TYPE,
        "config": descriptor(crate::oci::CONFIG_MEDIA_TYPE, EMPTY_CONFIG),
        "layers": [layer],
    }))
    .expect("a JSON document built from literals serialises");

    Ok(Signature {
        payload,
        signature,
        tag: format!("{}.sig", manifest_digest.replace(':', "-")),
        manifest,
        config: EMPTY_CONFIG.to_vec(),
    })
}

fn descriptor(media_type: &str, bytes: &[u8]) -> Value {
    json!({
        "mediaType": media_type,
        "digest": crate::oci::digest(bytes),
        "size": bytes.len(),
    })
}

/// PEM, at 64 characters a line — RFC 7468's form.
fn pem(label: &str, der: &[u8]) -> String {
    let body = base64(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn pem_body(text: &str, label: &str) -> Result<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body: String = text
        .lines()
        .skip_while(|l| l.trim() != begin)
        .skip(1)
        .take_while(|l| l.trim() != end)
        .collect();
    if body.is_empty() {
        bail!("no {label} block in the PEM");
    }
    unbase64(&body)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding — RFC 4648 §4, which is the alphabet PEM and cosign both use.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 0x3f]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unbase64(text: &str) -> Result<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = ALPHABET
            .iter()
            .position(|a| char::from(*a) == c)
            .with_context(|| format!("{c:?} is not a base64 character"))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10's vectors. A base64 written by hand is worth exactly as much as the vectors it
    /// was checked against, and every signature and every PEM below goes through it.
    #[test]
    fn base64_agrees_with_rfc_4648() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded, "encoding {plain:?}");
            assert_eq!(
                unbase64(encoded).expect("decodes"),
                plain.as_bytes(),
                "decoding {encoded:?}"
            );
        }
    }

    #[test]
    fn a_signature_verifies_against_the_public_key_alone() {
        let key = Key::generate().expect("a key");
        let digest = crate::oci::digest(b"a manifest");
        let signed = image(&key, "todo:dev", &digest).expect("signs");
        verify(&signed.payload, &signed.signature, &key.public_pem()).expect("verifies");
    }

    #[test]
    fn something_that_is_not_a_manifest_digest_is_refused() {
        let key = Key::generate().expect("a key");
        // A signature over a reference rather than a digest is the mistake worth refusing: a tag
        // moves, so a signature naming one asserts nothing about what is deployed.
        assert!(image(&key, "todo:dev", "todo:dev").is_err());
    }

    #[test]
    fn another_key_does_not_verify_it() {
        let (mine, theirs) = (Key::generate().expect("a"), Key::generate().expect("b"));
        let digest = crate::oci::digest(b"a manifest");
        let signed = image(&mine, "todo:dev", &digest).expect("signs");
        assert!(verify(&signed.payload, &signed.signature, &theirs.public_pem()).is_err());
    }

    #[test]
    fn a_changed_payload_does_not_verify() {
        let key = Key::generate().expect("a key");
        let signed = image(&key, "todo:dev", &crate::oci::digest(b"a manifest")).expect("signs");
        let tampered = payload("todo:dev", &crate::oci::digest(b"a different manifest"));
        assert!(verify(tampered.as_bytes(), &signed.signature, &key.public_pem()).is_err());
    }

    #[test]
    fn a_key_round_trips_through_its_own_pem() {
        let key = Key::generate().expect("a key");
        let back = Key::from_pem(&key.private_pem()).expect("reads back");
        assert_eq!(back.public_pem(), key.public_pem());
        let digest = crate::oci::digest(b"a manifest");
        let signed = image(&back, "todo:dev", &digest).expect("signs");
        verify(&signed.payload, &signed.signature, &key.public_pem()).expect("verifies");
    }

    #[test]
    fn the_payload_is_the_document_cosign_expects() {
        let text = payload("todo:dev", "sha256:abc");
        assert_eq!(
            text,
            "{\"critical\":{\"identity\":{\"docker-reference\":\"todo:dev\"},\
             \"image\":{\"docker-manifest-digest\":\"sha256:abc\"},\
             \"type\":\"cosign container image signature\"},\"optional\":null}"
        );
        // And it parses — the byte string above is also a JSON document, which is what a verifier
        // that reads the digest out of it needs.
        let value: Value = serde_json::from_str(&text).expect("parses");
        assert_eq!(
            value["critical"]["image"]["docker-manifest-digest"],
            "sha256:abc"
        );
    }
}
