"""Offline diagnostic of the normal managed Python API, using real vocabularies.

Run in a release-wheel environment: python -I python/bench_managed_ontology.py ROOT
ROOT must not exist. Results are JSON; fixtures and retained generations stay in ROOT.
This is a warm-cache diagnostic, not an SLO or a cold-storage benchmark.
"""

import hashlib
import importlib.metadata
import json
import resource
import shutil
import subprocess
import sys
from pathlib import Path
from time import perf_counter

import ontoenv
from ontoenv import OntoEnv
from rdflib import Literal, URIRef

REPO = Path(__file__).resolve().parents[1]
BRICK = "https://brickschema.org/schema/1.3/Brick"
MODEL = "urn:review:building:0"
REVISION = URIRef("urn:review:revision")


def timed(operation):
    start = perf_counter()
    result = operation()
    return result, (perf_counter() - start) * 1000


def memory():
    # Linux-specific current RSS/PSS plus process high-water mark.
    result = {"peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}
    for line in Path("/proc/self/smaps_rollup").read_text().splitlines():
        if line.startswith(("Rss:", "Pss:")):
            key, value, _ = line.split()
            result[key.rstrip(":").lower() + "_kib"] = int(value)
    return result


def model(number, nodes, revision=0):
    lines = [
        '@prefix owl: <http://www.w3.org/2002/07/owl#> .',
        '@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .',
        '@prefix brick: <https://brickschema.org/schema/Brick#> .',
        '@prefix qudt: <http://qudt.org/schema/qudt/> .',
        '@prefix unit: <http://qudt.org/vocab/unit/> .',
        f'<urn:review:building:{number}> a owl:Ontology; owl:imports <{BRICK}>;',
        f'  <urn:review:revision> "{revision}" .',
    ]
    for index in range(nodes):
        token = hashlib.sha256(f"{number}/{index}".encode()).hexdigest()
        lines.append(
            f'<urn:review:building:{number}:sensor:{token}> a brick:Temperature_Sensor; '
            f'rdfs:label "Building {number} zone {index} temperature"; '
            f'qudt:unit unit:DEG_C; '
            f'<urn:review:assetId> "{token}"; '
            f'brick:isPointOf <urn:review:building:{number}:ahu:{index // 10}>; '
            f'<urn:review:commissioningRecord> "'
            + hashlib.sha512(f"commissioned/{number}/{index}".encode()).hexdigest()
            + '" .'
        )
    return "\n".join(lines) + "\n"


def selected(root):
    directory = root / ".ontoenv-snapshots"
    head = json.loads((directory / "CURRENT").read_text())
    record = json.loads((directory / "manifests" / head["manifest"]).read_text())
    return record


def main():
    native_path = Path(ontoenv._native.__file__).resolve()
    if native_path.is_relative_to(REPO):
        raise RuntimeError("Use python -I to profile the installed wheel, not the local extension")
    root = Path(sys.argv[1]).resolve()
    nodes = int(sys.argv[2]) if len(sys.argv) > 2 else 350
    root.mkdir()  # Refuse reuse: each run has a new environment.
    sources = root / "sources"
    sources.mkdir()
    fixture = REPO / "lib/fixtures/brick-stuff"
    real = [fixture / "Brick-1.3.ttl", *sorted((fixture / "support").glob("*.ttl"))]
    for path in real:
        shutil.copyfile(path, sources / path.name)
    models = 100 - len(real)
    for number in range(models):
        (sources / f"building-{number}.ttl").write_text(model(number, nodes))
    report = {
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=REPO, text=True
        ).strip(),
        "working_tree_dirty": subprocess.run(
            ["git", "diff", "--quiet"], cwd=REPO, check=False
        ).returncode != 0,
        "native_extension_path": str(native_path),
        "package_version": importlib.metadata.version("ontoenv"),
        "python": sys.version,
        "nodes_per_model": nodes,
        "model_graphs": models,
        "real_vocabulary_files": len(real),
        "source_bytes": sum(p.stat().st_size for p in sources.iterdir()),
        "real_source_bytes": sum(p.stat().st_size for p in real),
        "source_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in sorted(sources.iterdir())},
        "ms": {},
        "memory": {"before_connect": memory()},
    }
    ms = report["ms"]
    writer, ms["connect_create_empty"] = timed(lambda: OntoEnv.connect(
        root, offline=True, strict=False, search_directories=[str(sources)],
    ))
    _, ms["initial_update_scan_parse_publish"] = timed(writer.update)
    report["memory"]["after_initial_update"] = memory()
    report["reported_missing_imports_during_scan"] = sorted(map(str, writer.missing_imports()))
    initial = selected(root)
    report["initial_object_bytes"] = initial["object_length"]
    assert 10 * 1024**2 <= initial["object_length"] <= 20 * 1024**2
    report["graphs"] = len(writer.get_ontology_names())
    report["initial_generation"] = writer.snapshot().info["generation"]
    assert report["graphs"] == 100
    warm_opens = []
    for _ in range(5):
        fresh, elapsed = timed(lambda: OntoEnv.open(root, read_only=True))
        warm_opens.append(elapsed)
        fresh.close()
    ms["warm_open_5"] = warm_opens
    report["memory"]["after_warm_opens"] = memory()
    reader = OntoEnv.open(root, read_only=True)
    report["unresolved_imports_after_reopen"] = sorted(map(str, reader.missing_imports()))
    old = reader.snapshot()
    view, ms["graph_view"] = timed(lambda: reader.get_graph(MODEL))
    iterator = view.triples((None, None, None))
    first, ms["first_triple"] = timed(lambda: next(iterator))
    report["memory"]["after_first_triple"] = memory()
    closure_result, ms["get_closure"] = timed(lambda: reader.get_closure(MODEL))
    closure, _ = closure_result
    query = '''SELECT ?s ?unit WHERE {
      ?s a <https://brickschema.org/schema/Brick#Temperature_Sensor> ;
         <http://qudt.org/schema/qudt/unit> ?unit . }'''
    rows, ms["first_closure_query_and_materialization"] = timed(lambda: list(closure.query(query)))
    assert len(rows) == nodes
    report["closure_query_rows"] = len(rows)
    ms["warm_closure_query_5"] = [timed(lambda: list(closure.query(query)))[1] for _ in range(5)]
    paths, ms["schema_path_query"] = timed(lambda: list(closure.query('''
      SELECT ?class WHERE { ?class <http://www.w3.org/2000/01/rdf-schema#subClassOf>+
        <https://brickschema.org/schema/Brick#Sensor> } LIMIT 100''')))
    report["schema_path_rows"] = len(paths)
    assert paths
    report["memory"]["after_closure_queries"] = memory()
    dataset = reader.get_dataset()
    cross, ms["cross_graph_query"] = timed(lambda: list(dataset.query('''
      SELECT ?g WHERE { GRAPH ?g { ?g a <http://www.w3.org/2002/07/owl#Ontology> } }''')))
    report["cross_graph_rows"] = len(cross)
    assert len(cross) >= models
    report["dataset_triples"] = len(dataset)
    report["memory"]["after_cross_graph_query"] = memory()
    before = writer.snapshot().info["generation"]
    _, ms["unchanged_update"] = timed(writer.update)
    assert writer.snapshot().info["generation"] == before
    source = sources / "building-0.ttl"
    source.write_text(model(0, nodes, 1))
    before = writer.snapshot().info["generation"]
    _, ms["one_source_update_imports_full_publication"] = timed(lambda: writer.update(source, force=True))
    report["memory"]["after_one_source_update"] = memory()
    assert writer.snapshot().info["generation"] == before + 1
    assert reader.snapshot().info["generation"] == old.info["generation"]
    assert (URIRef(MODEL), REVISION, Literal("0")) in view
    graph_object = selected(root)["object_id"]
    _, ms["alias_metadata_publication"] = timed(lambda: writer.add_alias("urn:review:alias", MODEL))
    assert selected(root)["object_id"] == graph_object
    changed, ms["reader_refresh"] = timed(reader.refresh)
    assert changed
    report["memory"]["after_refresh_retaining_old_views"] = memory()
    assert reader.resolve_alias("urn:review:alias") == MODEL
    assert (URIRef(MODEL), REVISION, Literal("1")) in reader.get_graph(MODEL)
    assert (URIRef(MODEL), REVISION, Literal("0")) in old.view_graph(MODEL)
    _, ms["explicit_dataset_rebind"] = timed(lambda: reader.refresh_dataset(dataset))
    assert (URIRef(MODEL), REVISION, Literal("1")) in dataset.graph(URIRef(MODEL))
    # An iterator created before publication/rebinding must finish its old graph.
    old_rows = {first, *iterator}
    assert len(old_rows) == nodes * 6 + 3
    assert (URIRef(MODEL), REVISION, Literal("0")) in old_rows
    report["old_iterator_rows"] = len(old_rows)
    writer.set_strict(True)
    writer.flush()
    before = writer.snapshot().info["generation"]
    source.write_text("this is not valid Turtle [")
    start = perf_counter()
    try:
        writer.update(source, force=True)
    except Exception as error:
        report["parse_failure"] = str(error)
    else:
        raise AssertionError("invalid source update succeeded")
    ms["failed_parse_rollback"] = (perf_counter() - start) * 1000
    assert writer.snapshot().info["generation"] == before
    assert selected(root)["info"]["generation"] == before
    writer.set_strict(False)
    writer.flush()
    before = writer.snapshot().info["generation"]
    source.write_text(model(0, nodes, 2))
    _, ms["retry_update_after_failure"] = timed(lambda: writer.update(source, force=True))
    assert writer.snapshot().info["generation"] == before + 1
    report["final_generation"] = writer.snapshot().info["generation"]
    report["retained_objects"] = [p.stat().st_size for p in
        sorted((root / ".ontoenv-snapshots/objects").glob("*/store.r5tu"))]
    report["retained_rdf_bytes"] = sum(report["retained_objects"])
    report["peak_rss_kib"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    reader.close()
    writer.close()
    assert (URIRef(MODEL), REVISION, Literal("0")) in view
    report["assertions"] = "passed: counts, query results, generations, retained views/iterator, alias, parse rollback, retry, close"
    (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
