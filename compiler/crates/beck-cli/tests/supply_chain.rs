//! The bill of materials, against the thing it describes.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) lists an SBOM among the supply-chain
//! tooling, and [`docs/92`](../../../../docs/92-sbom-report.md) is what building one found. An
//! inventory is a document that claims something about an artefact, so what these tests are for is
//! the claim rather than the format: **is it right about the image?**
//!
//! The first test is the one that matters. Everything else here would pass against an SBOM
//! assembled by hand beside the apko config and left to rot; that one fails the moment the two
//! disagree, because it reads the emitted YAML back and compares.

use std::collections::BTreeSet;

use beck_infra::sbom;

/// Compile, allowing a library — a program with no merge point is exactly the case the substrate
/// test needs, and `compile_str` refuses one.
fn compile(name: &str, src: &str) -> beck_core::Placed {
    let (placed, diags, map) = beck_core::compile_or_library_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

fn sketch() -> (beck_core::Placed, String) {
    let src = include_str!("../../../examples/todo.beck").to_string();
    (compile("examples/todo.beck", &src), src)
}

/// The `packages:` block of an apko config, as apko would read it.
fn apko_packages(yaml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in yaml.lines() {
        if line.trim_start().starts_with("packages:") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        match line.trim_start().strip_prefix("- ") {
            Some(name) => out.push(name.trim().to_string()),
            None if line.trim().is_empty() => break,
            None => break,
        }
    }
    out
}

#[test]
fn the_bill_of_materials_lists_what_the_image_config_installs() {
    // The whole point of the module. `sbom::packages` is the single source for both, so this reads
    // the *rendered* YAML back rather than calling the same function twice — a test that called
    // the function would agree with itself no matter what the config said.
    let (placed, _) = sketch();
    let graph = beck_infra::graph(&placed);
    let installed: BTreeSet<String> = apko_packages(&beck_infra::k8s::apko(&graph))
        .into_iter()
        .collect();
    assert!(
        !installed.is_empty(),
        "the apko config installs nothing; the parser above is reading the wrong block"
    );
    let listed: BTreeSet<String> = sbom::packages(&graph)
        .iter()
        .map(|p| p.apko_name())
        .collect();
    assert_eq!(
        listed, installed,
        "the bill of materials and the image config disagree about what is in the image"
    );
}

#[test]
fn the_melange_package_installs_the_files_the_in_process_build_writes() {
    // §92.2's rule, applied to paths rather than to package names. There are two routes to an
    // image — apko over a melange package, and `beck image` (docs/98) — and both have to put the
    // toolchain and the program in the same places. This reads the *rendered* melange YAML back,
    // so a route that started installing somewhere else fails here rather than in a container.
    let (placed, _) = sketch();
    let graph = beck_infra::graph(&placed);
    let melange = beck_infra::k8s::melange(&graph);

    let installed: BTreeSet<String> = melange
        .lines()
        .filter_map(|l| l.trim().strip_prefix("install -m"))
        .filter_map(|rest| rest.split('"').nth(1).map(str::to_string))
        .map(|dest| dest.replace("${{targets.destdir}}", ""))
        .collect();
    assert!(
        !installed.is_empty(),
        "the melange pipeline installs nothing; the parser above is reading the wrong block"
    );
    assert_eq!(
        installed,
        beck_infra::INSTALLS
            .iter()
            .map(|i| i.path.to_string())
            .collect::<BTreeSet<String>>(),
    );
    // …and with the modes, which is the half a path comparison would miss: a toolchain installed
    // 0644 is an image that cannot start.
    for i in beck_infra::INSTALLS {
        assert!(
            melange.contains(&format!("install -m{:o} {} ", i.mode, i.from)),
            "{} is not installed with mode {:o}:\n{melange}",
            i.path,
            i.mode
        );
    }
}

#[test]
fn the_image_config_and_the_pod_run_as_the_same_account() {
    // The mismatch this guards against is the classic "works locally, CrashLoopBackOff in the
    // cluster", and it is invisible in review because each file is right on its own. Both numbers
    // are read out of rendered output — the apko config's `accounts:` block and the Deployment's
    // `securityContext` — rather than out of the constant they are both rendered from.
    let (placed, _) = sketch();
    let graph = beck_infra::graph(&placed);
    let account = beck_infra::NONROOT;

    let apko = beck_infra::k8s::apko(&graph);
    assert!(
        apko.contains(&format!("run-as: {}", account.uid))
            && apko.contains(&format!("username: {}", account.user)),
        "{apko}"
    );

    let workload = beck_infra::k8s::objects(&graph, &placed.wire_id)
        .into_iter()
        .find(|(_, v)| v["kind"] == "Deployment")
        .map(|(_, v)| v)
        .expect("the sketch derives a Deployment");
    assert_eq!(
        workload["spec"]["template"]["spec"]["securityContext"]["runAsUser"], account.uid,
        "the pod and the image disagree about which user the program runs as"
    );
}

#[test]
fn the_config_names_only_architectures_the_builder_can_build() {
    // An apko config listing an architecture `beck image` cannot resolve a repository path for is
    // a config that describes an image nothing here can produce.
    let (placed, _) = sketch();
    let graph = beck_infra::graph(&placed);
    let apko = beck_infra::k8s::apko(&graph);
    let listed: Vec<String> = apko
        .lines()
        .skip_while(|l| !l.starts_with("archs:"))
        .skip(1)
        .take_while(|l| l.starts_with("  - "))
        .map(|l| l.trim_start_matches("  - ").to_string())
        .collect();
    assert!(!listed.is_empty(), "no archs block in:\n{apko}");
    for arch in &listed {
        beck_infra::oci::oci_arch(arch)
            .unwrap_or_else(|e| panic!("the config lists {arch}, and the builder says: {e}"));
    }
}

#[test]
fn every_dependency_names_a_component_that_is_there() {
    // A `dependsOn` pointing at a `bom-ref` no component declares is the way an SBOM is most often
    // wrong while still parsing, and every consumer of one resolves those refs.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let doc = sbom::cyclonedx(&graph, &src, &placed.wire_id);

    let mut refs: BTreeSet<&str> = doc["components"]
        .as_array()
        .expect("components is an array")
        .iter()
        .filter_map(|c| c["bom-ref"].as_str())
        .collect();
    refs.insert(
        doc["metadata"]["component"]["bom-ref"]
            .as_str()
            .expect("the metadata component has a ref"),
    );

    for edge in doc["dependencies"].as_array().expect("dependencies") {
        let from = edge["ref"].as_str().expect("an edge has a source");
        assert!(refs.contains(from), "`{from}` is not a component");
        for to in edge["dependsOn"].as_array().expect("dependsOn") {
            let to = to.as_str().expect("a ref is a string");
            assert!(refs.contains(to), "`{to}` is depended on and not declared");
        }
    }
}

#[test]
fn the_document_carries_what_a_consumer_requires() {
    // Not a schema validation — that would need the schema, and CycloneDX's is 4,000 lines of
    // JSON Schema this project has no reason to vendor. These are the fields every tool reads and
    // the ones CISA's minimum elements name: what it is, who made it, and a unique identifier.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let doc = sbom::cyclonedx(&graph, &src, &placed.wire_id);

    assert_eq!(doc["bomFormat"], "CycloneDX");
    assert_eq!(doc["specVersion"], "1.6");
    assert_eq!(doc["metadata"]["component"]["type"], "application");
    assert_eq!(doc["metadata"]["component"]["name"], "todo");
    assert_eq!(doc["metadata"]["tools"]["components"][0]["name"], "beck");

    let serial = doc["serialNumber"].as_str().expect("a serial number");
    assert!(serial.starts_with("urn:uuid:"), "{serial}");
    assert_eq!(serial.len(), "urn:uuid:".len() + 36, "{serial}");

    // The digest is of the program's own source, and it is checked by recomputing rather than by
    // reading it back — the latter passes for any string at all.
    assert_eq!(
        doc["metadata"]["component"]["hashes"][0]["content"],
        beck_core::digest::of(&src)
    );
    assert_eq!(doc["metadata"]["component"]["hashes"][0]["alg"], "BLAKE3");

    // The wire id is the contract this build serves, which is the fact that makes a bill of
    // materials answer "which build is deployed?" rather than only "what is in it?".
    let props = doc["metadata"]["component"]["properties"]
        .as_array()
        .expect("properties");
    let wire = props
        .iter()
        .find(|p| p["name"] == "beck:wire-id")
        .expect("the wire id is a property");
    assert_eq!(wire["value"], placed.wire_id);
}

#[test]
fn two_builds_of_one_program_produce_the_same_document() {
    // Reproducibility is the property §6.2's image config exists for, and an SBOM with a timestamp
    // or a fresh UUID in it cannot be compared that way — so "did the bill of materials change?"
    // is answerable by `cmp`.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let a = sbom::render(&graph, &src, &placed.wire_id);
    let b = sbom::render(&graph, &src, &placed.wire_id);
    assert_eq!(a, b);
    assert!(
        !a.contains("timestamp"),
        "a timestamp makes two builds incomparable:\n{a}"
    );
}

#[test]
fn a_changed_program_gets_a_different_serial_number() {
    // The other half of the same claim: identical documents for identical inputs is worthless if
    // the identifier does not move when the input does.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let one = sbom::cyclonedx(&graph, &src, &placed.wire_id);
    let two = sbom::cyclonedx(&graph, &format!("{src}\n# a comment\n"), &placed.wire_id);
    assert_ne!(one["serialNumber"], two["serialNumber"]);
}

#[test]
fn the_substrate_is_listed_exactly_when_a_fold_derived_one() {
    // A component the deployment starts and the image does not contain. It is in the document
    // because a bill of materials that covered only the app's own image would omit the database —
    // and it is *derived*, so a program with no durable fold must not claim one.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let with = sbom::render(&graph, &src, &placed.wire_id);
    assert!(with.contains("postgres"), "{with}");

    let library = "def double(n: Int) -> Int:\n    return n * 2\n";
    let placed = compile("lib.beck", library);
    let graph = beck_infra::graph(&placed);
    let without = sbom::render(&graph, library, &placed.wire_id);
    assert!(
        !without.contains("postgres"),
        "a program with no durable fold claims a database:\n{without}"
    );
}

#[test]
fn the_standard_library_a_program_imports_is_listed() {
    // `import bignum` is a dependency even though nothing downloads it: it is code this project
    // ships, compiled into the binary, and "is this program affected?" is a question somebody will
    // ask about it. Both directions, because listing every module always would pass the first.
    //
    // The import is read from the *source*, so this uses the sketch's graph with a source that
    // has one — `beck-core`'s string entry points do not resolve an import against the standard
    // library, which is a property of `beck check <file>` rather than of the compiler's API.
    let (placed, src) = sketch();
    let graph = beck_infra::graph(&placed);
    let doc = sbom::render(&graph, &format!("import bignum\n{src}"), &placed.wire_id);
    assert!(doc.contains("pkg:generic/beck/bignum"), "{doc}");
    assert!(
        !doc.contains("pkg:generic/beck/crypto"),
        "a module the program does not import is listed:\n{doc}"
    );

    // A name that is not a standard-library module is not one: `import helpers` beside the file is
    // the program's own code, and an inventory that called it a dependency of this project would
    // be wrong about who ships it.
    let doc = sbom::render(&graph, &format!("import helpers\n{src}"), &placed.wire_id);
    assert!(!doc.contains("beck/helpers"), "{doc}");
}

#[test]
fn beck_build_writes_one_beside_the_manifests() {
    // An artefact that has to be asked for separately is one that goes stale, so `beck build`
    // writes it. This is also the only test here that goes through the emitter end to end.
    let dir = std::env::temp_dir().join("beck-sbom-build");
    let _ = std::fs::remove_dir_all(&dir);
    let (placed, src) = sketch();
    let written = beck_infra::emit(&placed, &src, &dir).expect("the emitter writes");
    let path = dir.join(beck_infra::SBOM_FILE);
    assert!(
        written.contains(&path),
        "`beck build` did not write a bill of materials: {written:?}"
    );
    let body = std::fs::read_to_string(&path).expect("it is readable");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("it is JSON");
    assert_eq!(parsed["bomFormat"], "CycloneDX");
}
