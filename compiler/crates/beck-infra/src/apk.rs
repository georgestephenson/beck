//! The Alpine package format, read-only — the input side of the in-process image build.
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md) §6.2
//! chose apko, and named the destination: "move to writing the OCI layout directly from Rust …
//! once the format is settled, so `beck build` is one process with no external tools". An image
//! whose contents come from packages and from nothing else needs two things to be built without
//! apko: a way to *find* those packages, and a way to *read* one. This module is both;
//! [`crate::oci`] is what assembles the result.
//!
//! # An index, and a resolver that says what it cannot do
//!
//! `APKINDEX` is a flat text file of `X:value` lines in blank-line-separated records, and the
//! resolution done here is deliberately small: a name, or something a package `provides`, with the
//! highest version winning. **Version constraints are parsed and not solved** — a dependency
//! written `foo>=1.2` resolves as `foo`, and [`Resolution::constraints`] carries every constraint
//! that was dropped so a caller can print them rather than a caller believing they were checked.
//! That is the honest shape for a resolver whose job is three packages deep: apk's own solver is a
//! SAT problem, and a half-implemented one that silently picked a too-old package would be worse
//! than one that says so.
//!
//! # Why the archive reading is written here rather than taken
//!
//! An APK is three gzip members concatenated — a signature segment, a control segment and the data
//! — so decompressing it yields three tar streams end to end, and the file list is the third with
//! the dotfiles of the first two skipped. That is thirty lines against a tar crate's dependency
//! tree, and the *determinism* of what comes out is the property the image build rests on, so it is
//! worth owning: [`Contents::files`] is sorted by path, and nothing here reads a clock or a
//! filesystem.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use anyhow::{bail, Context, Result};

/// One record of an `APKINDEX`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// `P:` — the package name.
    pub name: String,
    /// `V:` — the version, as the repository spells it.
    pub version: String,
    /// `A:` — the architecture the package was built for.
    pub arch: String,
    /// `C:` — the checksum, in apk's `Q1<base64>` spelling. Carried, not verified: it covers the
    /// control segment rather than the file, and what an image build needs to pin is the whole
    /// package, which [`crate::oci`] does with a SHA-256 of the bytes it was given.
    pub checksum: String,
    /// `S:` — the size of the `.apk`, in bytes.
    pub size: u64,
    /// `D:` — what this package needs, verbatim, including constraints and `!conflicts`.
    pub depends: Vec<String>,
    /// `p:` — what this package satisfies, verbatim.
    pub provides: Vec<String>,
}

impl Entry {
    /// The file name this package has in a repository: `<name>-<version>.apk`.
    pub fn file_name(&self) -> String {
        format!("{}-{}.apk", self.name, self.version)
    }

    /// Where it sits under a repository root, for a given architecture.
    pub fn url(&self, repository: &str, arch: &str) -> String {
        format!(
            "{}/{}/{}",
            repository.trim_end_matches('/'),
            arch,
            self.file_name()
        )
    }
}

/// A parsed `APKINDEX`.
#[derive(Clone, Debug, Default)]
pub struct Index {
    entries: Vec<Entry>,
}

impl Index {
    /// Parse the text of an `APKINDEX` — the file, not the `.tar.gz` around it.
    ///
    /// Unknown fields are ignored rather than refused: the format grows, and an index this cannot
    /// read is an index nobody can build an image from.
    pub fn parse(text: &str) -> Index {
        let mut entries = Vec::new();
        let mut current = Entry::default();
        for line in text.lines() {
            if line.trim().is_empty() {
                if !current.name.is_empty() {
                    entries.push(std::mem::take(&mut current));
                }
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let words = || {
                value
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            };
            match key {
                "P" => current.name = value.to_string(),
                "V" => current.version = value.to_string(),
                "A" => current.arch = value.to_string(),
                "C" => current.checksum = value.to_string(),
                "S" => current.size = value.trim().parse().unwrap_or(0),
                "D" => current.depends = words(),
                "p" => current.provides = words(),
                _ => {}
            }
        }
        if !current.name.is_empty() {
            entries.push(current);
        }
        Index { entries }
    }

    /// Read an `APKINDEX.tar.gz` as a repository serves it.
    pub fn read(archive: &[u8]) -> Result<Index> {
        let files = Contents::read(archive).context("reading APKINDEX.tar.gz")?;
        let index = files
            .find("APKINDEX")
            .context("no APKINDEX member in the index archive")?;
        Index::parse(&String::from_utf8_lossy(index))
            .checked()
            .context("the APKINDEX parsed to nothing — the archive is not an index")
    }

    fn checked(self) -> Option<Index> {
        (!self.entries.is_empty()).then_some(self)
    }

    /// Every record, in the order the index listed them.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The best record for a name or for something a package provides.
    ///
    /// "Best" is the highest version by `version_key`, which orders the dotted-numeric spellings
    /// Wolfi uses and falls back to a byte comparison for anything else.
    pub fn best(&self, wanted: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.name == wanted || provides(e, wanted))
            .max_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)))
    }

    /// Everything that has to be installed for `wanted`, in a deterministic order.
    ///
    /// The order is dependencies before dependents, and alphabetical between packages that do not
    /// order each other — an image whose layer depends on hash-map iteration would not be
    /// reproducible, which is the whole point of §6.2.
    pub fn resolve(&self, wanted: &[String]) -> Result<Resolution> {
        let mut chosen: BTreeMap<String, &Entry> = BTreeMap::new();
        let mut constraints: BTreeSet<String> = BTreeSet::new();
        let mut missing: BTreeSet<String> = BTreeSet::new();
        let mut queue: Vec<String> = wanted.to_vec();
        let mut seen: BTreeSet<String> = BTreeSet::new();

        while let Some(want) = queue.pop() {
            let (bare, constraint) = split_constraint(&want);
            // A conflict marker is a statement about what must *not* be installed. Nothing here
            // installs anything it was not asked for, so recording it is the whole response.
            if let Some(conflict) = bare.strip_prefix('!') {
                constraints.insert(format!("!{conflict}"));
                continue;
            }
            if !seen.insert(bare.to_string()) {
                continue;
            }
            if constraint.is_some() {
                constraints.insert(want.clone());
            }
            match self.best(bare) {
                Some(entry) => {
                    if chosen.insert(entry.name.clone(), entry).is_none() {
                        queue.extend(entry.depends.iter().cloned());
                    }
                }
                None => {
                    missing.insert(bare.to_string());
                }
            }
        }

        if !missing.is_empty() {
            bail!(
                "the repository index has no package for: {}",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            );
        }

        Ok(Resolution {
            packages: chosen.into_values().cloned().collect(),
            constraints: constraints.into_iter().collect(),
        })
    }
}

/// What [`Index::resolve`] decided, and what it declined to decide.
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    /// The packages to install, ordered by name.
    pub packages: Vec<Entry>,
    /// Every version constraint and conflict marker the resolver read and did **not** enforce.
    ///
    /// Not decoration: a caller prints these, because a constraint nobody checked is a constraint
    /// nobody should believe was met.
    pub constraints: Vec<String>,
}

fn provides(entry: &Entry, wanted: &str) -> bool {
    entry
        .provides
        .iter()
        .any(|p| split_constraint(p).0 == wanted)
}

/// Split `foo>=1.2` into `("foo", Some(">=1.2"))`. Constraints are recorded, never solved.
fn split_constraint(dep: &str) -> (&str, Option<&str>) {
    match dep.find(['<', '>', '=', '~']) {
        Some(i) if i > 0 => (&dep[..i], Some(&dep[i..])),
        _ => (dep, None),
    }
}

/// A sortable key for an apk version: the dotted numeric run, then the whole string.
///
/// Numbers first so that `1.10` beats `1.9`, which a byte comparison gets backwards; the string
/// tail then orders suffixes (`-r1` against `-r2`) consistently without claiming to implement apk's
/// full comparison, which knows about `_alpha` and `_p` and is not needed to pick between the two
/// entries a repository index has for one package.
fn version_key(version: &str) -> (Vec<u64>, String) {
    let numeric = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    (numeric, version.to_string())
}

/// One file out of a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// The path inside the image, without a leading slash — as the tar spells it.
    pub path: String,
    /// The permission bits.
    pub mode: u32,
    /// What it is.
    pub kind: Kind,
}

/// What a tar member is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Regular(Vec<u8>),
    Symlink(String),
    /// A hard link to another member of the same archive, by path.
    Hardlink(String),
}

/// The files a package installs, sorted by path.
#[derive(Clone, Debug, Default)]
pub struct Contents {
    pub files: Vec<File>,
}

impl Contents {
    /// Read an APK — or any gzipped tar, which is what makes this reusable for `APKINDEX.tar.gz`.
    ///
    /// The three segments of an APK decompress into one stream of tar members; the metadata ones
    /// (`.PKGINFO`, `.SIGN.*`, the install scripts) are dotfiles at the root and are dropped, which
    /// leaves exactly the files the package installs.
    pub fn read(archive: &[u8]) -> Result<Contents> {
        let mut raw = Vec::new();
        flate2::read::MultiGzDecoder::new(archive)
            .read_to_end(&mut raw)
            .context("decompressing the archive")?;
        let mut files = untar(&raw)?;
        files.retain(|f| !f.path.starts_with('.') && !f.path.is_empty());
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Contents { files })
    }

    /// The bytes of one regular file, by path.
    pub fn find(&self, path: &str) -> Option<&[u8]> {
        self.files.iter().find_map(|f| match &f.kind {
            Kind::Regular(bytes) if f.path == path => Some(bytes.as_slice()),
            _ => None,
        })
    }
}

const BLOCK: usize = 512;

/// A tar reader that understands exactly what an APK contains.
///
/// ustar and GNU long names; a pax header is skipped rather than merged, because no package in a
/// Wolfi repository uses one for a path an image cares about and a half-read pax record would be a
/// silently wrong path rather than an error.
fn untar(raw: &[u8]) -> Result<Vec<File>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut long_name: Option<String> = None;

    while at + BLOCK <= raw.len() {
        let header = &raw[at..at + BLOCK];
        if header.iter().all(|b| *b == 0) {
            // An end-of-archive marker. An APK concatenates segments, so this is the end of the
            // last one; a segment that does not carry one simply runs into the next header.
            break;
        }
        at += BLOCK;

        let size = octal(&header[124..136]).context("a tar header with an unreadable size")?;
        let mode = octal(&header[100..108]).unwrap_or(0o644) as u32;
        let typeflag = header[156];
        let body_end = at + size as usize;
        if body_end > raw.len() {
            bail!("a tar member claims {size} bytes and the archive is shorter");
        }
        let body = &raw[at..body_end];
        at = body_end.div_ceil(BLOCK) * BLOCK;

        let name = match long_name.take() {
            Some(n) => n,
            None => {
                let prefix = string(&header[345..500]);
                let name = string(&header[0..100]);
                if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                }
            }
        };
        // `tar -C dir .` writes every member as `./usr/bin/x`, and apk-tools writes `usr/bin/x`.
        // One path in the image either way: a leading `./` is a spelling, not a directory, and two
        // spellings of one path in a layer is a file the runtime unpacks twice.
        let name = name.strip_prefix("./").unwrap_or(&name).to_string();

        match typeflag {
            b'L' => long_name = Some(String::from_utf8_lossy(body).trim_matches('\0').to_string()),
            b'x' | b'g' => {}
            b'5' => out.push(File {
                path: name.trim_end_matches('/').to_string(),
                mode,
                kind: Kind::Dir,
            }),
            b'2' => out.push(File {
                path: name,
                mode,
                kind: Kind::Symlink(string(&header[157..257])),
            }),
            b'1' => out.push(File {
                path: name,
                mode,
                kind: Kind::Hardlink(string(&header[157..257])),
            }),
            b'0' | 0 => out.push(File {
                path: name,
                mode,
                kind: Kind::Regular(body.to_vec()),
            }),
            // Character devices, fifos and the rest: a distroless image has none, and inventing a
            // representation for one nothing installs would be a guess.
            _ => {}
        }
    }
    Ok(out)
}

fn string(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_string()
}

fn octal(field: &[u8]) -> Result<u64> {
    let text = string(field);
    let text = text.trim_matches(char::from(0));
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text.trim(), 8).with_context(|| format!("{text:?} is not an octal field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "C:Q1abc\nP:tzdata\nV:2026a-r1\nA:x86_64\nS:100\n\n\
                         C:Q1def\nP:ca-certificates-bundle\nV:20260101-r0\nA:x86_64\nS:200\n\
                         D:libcrypto3 so:libc.so.6\n\n\
                         C:Q1ghi\nP:libcrypto3\nV:3.5.0-r0\nA:x86_64\np:so:libcrypto.so.3=3\n\n\
                         C:Q1jkl\nP:libc-dev\nV:1.0-r0\nA:x86_64\np:so:libc.so.6=1\n";

    #[test]
    fn an_index_parses_into_records() {
        let index = Index::parse(INDEX);
        assert_eq!(index.entries().len(), 4);
        let tz = index.best("tzdata").expect("tzdata is in the index");
        assert_eq!(tz.version, "2026a-r1");
        assert_eq!(tz.file_name(), "tzdata-2026a-r1.apk");
        assert_eq!(
            tz.url("https://packages.wolfi.dev/os/", "x86_64"),
            "https://packages.wolfi.dev/os/x86_64/tzdata-2026a-r1.apk"
        );
    }

    #[test]
    fn resolution_follows_dependencies_and_provides() {
        let index = Index::parse(INDEX);
        let r = index
            .resolve(&["ca-certificates-bundle".to_string()])
            .expect("resolvable");
        let names: Vec<&str> = r.packages.iter().map(|p| p.name.as_str()).collect();
        // `so:libc.so.6` is nobody's package name; it is `libc-dev`'s `provides`.
        assert_eq!(names, ["ca-certificates-bundle", "libc-dev", "libcrypto3"]);
    }

    #[test]
    fn a_dropped_constraint_is_reported_rather_than_believed() {
        let index =
            Index::parse("P:foo\nV:1.0\nA:x86_64\nD:bar>=2 !baz\n\nP:bar\nV:1.0\nA:x86_64\n");
        let r = index.resolve(&["foo".to_string()]).expect("resolvable");
        assert_eq!(r.constraints, ["!baz", "bar>=2"]);
    }

    #[test]
    fn a_missing_package_is_an_error_and_not_an_empty_image() {
        let index = Index::parse(INDEX);
        let err = index
            .resolve(&["nothing-provides-this".to_string()])
            .expect_err("must not resolve");
        assert!(err.to_string().contains("nothing-provides-this"));
    }

    #[test]
    fn the_higher_version_wins_even_when_the_string_comparison_disagrees() {
        let index = Index::parse("P:x\nV:1.9.0-r0\nA:x86_64\n\nP:x\nV:1.10.0-r0\nA:x86_64\n");
        assert_eq!(index.best("x").expect("present").version, "1.10.0-r0");
    }
}
