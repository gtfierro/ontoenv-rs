import pytest
import ontoenv
from ontoenv import OntoEnv, Config
import pathlib
import shutil
import os


# Fixture to create a temporary directory for each test
@pytest.fixture
def temp_dir(tmp_path):
    """Provides a temporary directory path for tests."""
    yield tmp_path
    # Cleanup happens automatically via pytest's tmp_path fixture


# Fixture to create a temporary directory with a pre-initialized OntoEnv
@pytest.fixture
def existing_env_dir(tmp_path):
    """Provides a temporary directory path with an initialized OntoEnv."""
    env_path = tmp_path / "existing_env"
    env_path.mkdir()
    # Use temporary=False explicitly if needed, ensure root is set
    cfg = Config(root=str(env_path), temporary=False)
    env = OntoEnv(config=cfg, path=env_path, recreate=True)
    # Add a dummy file to ensure the env is not empty if needed later
    # For now, just initializing is enough to create the .ontoenv structure
    env.flush()  # Ensure data is written if not temporary
    del env
    yield env_path
    # Cleanup happens automatically via pytest's tmp_path fixture


def test_init_with_config_new_dir(temp_dir):
    """Test initializing OntoEnv with a Config in a new directory."""
    env_path = temp_dir / "new_env"
    # Ensure the directory does not exist initially
    assert not env_path.exists()
    cfg = Config(root=str(env_path), temporary=False)
    env = OntoEnv(config=cfg, path=env_path, recreate=True)
    assert (env_path / ".ontoenv").is_dir()
    assert (
        env.store_path() is not None
    )  # Assuming store_path handles non-temporary envs


def test_init_with_config_existing_empty_dir(temp_dir):
    """Test initializing OntoEnv with a Config in an existing empty directory."""
    env_path = temp_dir / "empty_env"
    env_path.mkdir()
    assert env_path.is_dir()
    cfg = Config(root=str(env_path), temporary=False)
    env = OntoEnv(config=cfg, path=env_path, recreate=True)
    assert (env_path / ".ontoenv").is_dir()
    assert env.store_path() is not None


def test_init_load_from_existing_dir(existing_env_dir):
    """Test initializing OntoEnv by loading from an existing directory."""
    assert (existing_env_dir / ".ontoenv").is_dir()
    # Initialize by path only, should load existing
    env = OntoEnv(path=existing_env_dir, read_only=False)
    # Simple check: does it have a store path?
    assert env.store_path() == str(existing_env_dir / ".ontoenv" / "store.db")
    # Add more checks if the fixture pre-populates data


def test_init_recreate_existing_dir(existing_env_dir):
    """Test initializing OntoEnv with recreate=True on an existing directory."""
    assert (existing_env_dir / ".ontoenv").is_dir()
    # Optionally: Add a dummy file inside .ontoenv to check if it gets wiped
    (existing_env_dir / ".ontoenv" / "dummy.txt").touch()
    assert (existing_env_dir / ".ontoenv" / "dummy.txt").exists()

    # Recreate the environment
    cfg = Config(root=str(existing_env_dir), temporary=False)
    env = OntoEnv(config=cfg, path=existing_env_dir, recreate=True)

    assert (existing_env_dir / ".ontoenv").is_dir()
    # Check if the dummy file is gone (or check if ontology list is empty)
    assert not (existing_env_dir / ".ontoenv" / "dummy.txt").exists()
    assert len(env.get_ontology_names()) == 0


# Note: This test assumes add() raises an error for read-only mode.
# The Rust ReadOnlyPersistentGraphIO::add returns Err, which should map to PyErr.
def test_init_read_only(existing_env_dir):
    """Test initializing OntoEnv with read_only=True."""
    env = OntoEnv(path=existing_env_dir, read_only=True)
    assert (existing_env_dir / ".ontoenv").is_dir()

    # Attempting to modify should fail
    with pytest.raises(ValueError, match="Cannot add to read-only store"):
        # Use a dummy file path or URL
        env.add("file:///dummy.ttl")


def test_init_no_config_no_path_error():
    """Test initializing OntoEnv without config or valid path fails."""
    # Assuming current dir '.' does not contain a valid .ontoenv
    # Clean up potential leftover .ontoenv in cwd just in case
    if os.path.exists(".ontoenv"):
        if os.path.isfile(".ontoenv"):
            os.remove(".ontoenv")
        else:
            shutil.rmtree(".ontoenv")

    # Expecting failure because '.' likely doesn't contain a valid .ontoenv
    with pytest.raises(ValueError, match="OntoEnv directory not found at: \"./.ontoenv\""):
        OntoEnv()  # No args


def test_init_path_no_env_error(temp_dir):
    """Test initializing OntoEnv with a path to a dir without .ontoenv fails."""
    env_path = temp_dir / "no_env_here"
    env_path.mkdir()
    assert not (env_path / ".ontoenv").exists()
    # Expecting failure because the specified path doesn't contain a .ontoenv dir
    absolute_path = (env_path / ".ontoenv").resolve()
    with pytest.raises(ValueError, match=f"OntoEnv directory not found at: \"{absolute_path}\""):
        # This fails because load_from_directory expects .ontoenv unless recreate=True
        OntoEnv(path=env_path)


def test_init_temporary(temp_dir):
    """Test initializing OntoEnv with temporary=True."""
    env_path = temp_dir / "temp_env_root"
    # temporary envs don't persist to disk relative to root
    cfg = Config(root=str(env_path), temporary=True, strict=False)
    env = OntoEnv(config=cfg)  # Path shouldn't matter for temporary

    # .ontoenv directory should NOT be created at the root
    assert not (env_path / ".ontoenv").exists()

    # store_path() should indicate it's not persistent.
    # store_path() should return None for temporary envs
    assert env.store_path() is None

    # Check if adding works in memory (should not raise read-only error)
    # Note: Adding a URL might fail if offline=True by default or network issues
    # Adding a non-existent file path will fail regardless.
    # We'll just check that the *attempt* doesn't raise a read-only error.
    try:
        # Use a dummy URL that won't resolve but tests the add path
        env.add("http://example.com/nonexistent.ttl")
    except ValueError as e:
        # We expect errors related to fetching/reading, *not* read-only errors
        assert "Cannot add to read-only store" not in str(e)
    except Exception:
        # Catch other potential errors (like network) during add
        pass


# TODO: Add tests for offline mode, different resolution policies, includes/excludes etc.
