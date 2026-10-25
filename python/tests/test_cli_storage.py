"""Exercise the shipped CLI against the same default backend as Python."""
import json
import os
import subprocess
import sys
from pathlib import Path

from ontoenv import OntoEnv


def cli(root: Path, *args: str, success: bool = True, working_directory: Path | None = None):
    result = subprocess.run(
        [sys.executable, "-c", "from ontoenv import run_cli; import sys; raise SystemExit(run_cli(['ontoenv', *sys.argv[1:]]))", *args],
        cwd=working_directory or root, env={**os.environ, "ONTOENV_DIR": str(root)},
        capture_output=True, text=True, timeout=30,
    )
    assert (result.returncode == 0) == success, (result.stdout, result.stderr)
    return result


def test_cli_init_queries_configuration_and_reset_share_managed_storage(tmp_path: Path):
    source = tmp_path / "ontology.ttl"
    source.write_text('<urn:cli> a <http://www.w3.org/2002/07/owl#Ontology> .')
    cli(tmp_path, "init", "--offline")
    assert (tmp_path / ".ontoenv-snapshots/CURRENT").exists()
    assert not (tmp_path / ".ontoenv").exists()
    writer = OntoEnv.open(tmp_path)
    old = writer.snapshot()
    assert old.get_ontology_names() == ["urn:cli"]
    # Readers open despite the writer lock, and cannot accidentally write.
    status = json.loads(cli(tmp_path, "status", "--json").stdout)
    assert status["num_ontologies"] == 1
    cli(tmp_path, "config", "set", "strict", "true", success=False)
    assert writer.snapshot().info == old.info
    writer.close()
    cli(tmp_path, "config", "set", "strict", "true")
    elsewhere = tmp_path / "other-environment"
    other = OntoEnv.create(elsewhere, offline=True)
    other_info = other.snapshot().info
    other.close()
    cli(tmp_path, "config", "bind", "chosen", "urn:chosen:", working_directory=elsewhere)
    reader = OntoEnv.open(tmp_path, read_only=True)
    selected = reader.snapshot()
    assert selected.config["strict"] is True
    assert selected.get_bound_namespaces()["chosen"] == "urn:chosen:"
    cli(tmp_path, "reset", "--force", working_directory=elsewhere)
    other = OntoEnv.open(elsewhere, read_only=True)
    assert other.snapshot().info["generation"] == other_info["generation"]
    assert other.get_bound_namespaces() == {}
    other.close()
    assert reader.get_ontology_names() == ["urn:cli"]
    assert reader.refresh() is True
    assert reader.get_ontology_names() == []
    assert len(old.view_graph("urn:cli")) == 1
    reader.close()


def test_config_changes_preserve_deliberately_unfetched_imports(tmp_path: Path):
    source = tmp_path / "ontology.ttl"
    source.write_text('<urn:bounded> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <https://example.invalid/unfetched> .')
    env = OntoEnv.create(tmp_path, offline=True)
    env.add(str(source), fetch_imports=False)
    env.close()
    cli(tmp_path, "config", "set", "strict", "true")
    cli(tmp_path, "config", "bind", "chosen", "urn:chosen:")
    env = OntoEnv.open(tmp_path, read_only=True)
    assert env.is_strict()
    assert env.get_ontology_names() == ["urn:bounded"]
    assert env.snapshot().get_bound_namespaces()["chosen"] == "urn:chosen:"
    env.close()
