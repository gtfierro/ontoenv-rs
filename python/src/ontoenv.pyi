from pathlib import Path
from typing import Optional, List, Union, Any
from rdflib import Graph, Dataset

class Config:
    """
    Configuration class for setting up the ontology environment.

    Attributes:
        search_directories: Optional list of directories to search for ontologies.
        require_ontology_names: Flag to require ontology names.
        strict: Flag for strict mode.
        offline: Flag to operate in offline mode.
        resolution_policy: Policy for resolving ontologies.
        root: Root directory for the environment.
        includes: Optional list of patterns to include.
        excludes: Optional list of patterns to exclude.
        temporary: Flag to create a temporary environment.
        no_search: Flag to disable searching for ontologies in local directories.
    """
    def __init__(
        self,
        search_directories: Optional[List[str]] = None,
        require_ontology_names: bool = False,
        strict: bool = False,
        offline: bool = False,
        resolution_policy: str = "default",
        root: str = ".",
        includes: Optional[List[str]] = None,
        excludes: Optional[List[str]] = None,
        temporary: bool = False,
        no_search: bool = False,
    ) -> None:
        """
        Initialize the Config object with the given parameters.
        """
        ...

class OntoEnv:
    """
    Ontology Environment class for managing ontologies.

    Attributes:
        config: Optional configuration object.
        path: Path to the ontology environment.
        recreate: Flag to recreate the environment.
        read_only: Flag to set the environment as read-only.
    """
    def __init__(
        self,
        config: Optional[Config] = None,
        path: Optional[Union[str, Path]] = None,
        recreate: bool = False,
        read_only: bool = False,
    ) -> None:
        """
        Initialize the OntoEnv object with the given parameters.
        """
        ...

    def update(self) -> None:
        """
        Update the ontology environment by reloading all ontologies.
        """
        ...

    def __repr__(self) -> str:
        """
        Return a string representation of the OntoEnv object.
        """
        ...

    def import_graph(self, destination_graph: Any, uri: str) -> None:
        """
        Import a graph from the given URI into the destination graph.

        Args:
            destination_graph: The graph to import into.
            uri: The URI of the graph to import.
        """
        ...

    def list_closure(self, uri: str, recursion_depth: int = -1) -> List[str]:
        """
        List the ontologies in the imports closure of the given ontology.

        Args:
            uri: The URI of the ontology.
            recursion_depth: The maximum depth for recursive import resolution.
        Returns:
            A list of ontology names in the closure.
        """
        ...

    def get_closure(
        self,
        uri: str,
        destination_graph: Optional[Any] = None,
        rewrite_sh_prefixes: bool = True,
        remove_owl_imports: bool = True,
        recursion_depth: int = -1,
    ) -> tuple[Any, List[str]]:
        """
        Merge all graphs in the imports closure of the given ontology into a single graph.

        Args:
            uri: The URI of the ontology.
            destination_graph: Optional graph to add the merged graph to.
            rewrite_sh_prefixes: Flag to rewrite SH prefixes.
            remove_owl_imports: Flag to remove OWL imports.
            recursion_depth: The maximum depth for recursive import resolution.
        Returns:
            A tuple containing the merged graph and a list of ontology names in the closure.
        """
        ...

    def dump(self, includes: Optional[str] = None) -> None:
        """
        Print the contents of the OntoEnv.

        Args:
            includes: Optional string to filter the output.
        """
        ...

    def import_dependencies(self, graph: Any, recursion_depth: int = -1, fetch_missing: bool = False) -> List[str]:
        """
        Import the dependencies of the given graph into the graph.

        Args:
            graph: The graph to import dependencies into.
            recursion_depth: The maximum depth for recursive import resolution.
            fetch_missing: If True, will fetch ontologies that are not in the environment.
        Returns:
            A list of imported ontology names.
        """
        ...

    def get_dependencies_graph(
        self,
        graph: Any,
        destination_graph: Optional[Any] = None,
        recursion_depth: int = -1,
        fetch_missing: bool = False,
        rewrite_sh_prefixes: bool = True,
        remove_owl_imports: bool = True,
    ) -> tuple[Any, List[str]]:
        """
        Get the dependency closure of a given graph and return it as a new graph.

        This method will look for `owl:imports` statements in the provided `graph`,
        then find those ontologies within the `OntoEnv` and compute the full
        dependency closure. The triples of all ontologies in the closure are
        returned as a new graph. The original graph is not modified.

        Args:
            graph: The graph to find dependencies for.
            destination_graph: If provided, the dependency graph will be added to this
                graph instead of creating a new one.
            recursion_depth: The maximum depth for recursive import resolution. A
                negative value (default) means no limit.
            fetch_missing: If True, will fetch ontologies that are not in the environment.
            rewrite_sh_prefixes: If True, will rewrite SHACL prefixes to be unique.
            remove_owl_imports: If True, will remove `owl:imports` statements from the
                returned graph.

        Returns:
            A tuple containing the graph of dependencies and a list of the URIs of the
            imported ontologies.
        """
        ...

    def add(self, location: Any, overwrite: bool = False, fetch_imports: bool = True) -> str:
        """
        Add a new ontology to the OntoEnv.

        Args:
            location: The location of the ontology to add (file path or URL).
            overwrite: If True, will overwrite an existing ontology at the same location.
            fetch_imports: If True, will recursively fetch missing owl:imports.
        Returns:
            The URI string of the added ontology.
        """
        ...

    def add_no_imports(self, location: Any) -> str:
        """
        Add a new ontology to the OntoEnv without exploring owl:imports.

        Args:
            location: The location of the ontology to add (file path, URL, or rdflib.Graph).
        Returns:
            The URI string of the added ontology.
        """
        ...

    def get_importers(self, uri: str) -> List[str]:
        """
        Get the names of all ontologies that import the given ontology.

        Args:
            uri: The URI of the ontology.
        Returns:
            A list of ontology names that import the given ontology.
        """
        ...

    def get_graph(self, uri: str) -> Graph:
        """
        Get the graph with the given URI as an rdflib.Graph.

        Args:
            uri: The URI of the graph to get.
        Returns:
            An rdflib.Graph object representing the requested graph.
        """
        ...

    def get_ontology_names(self) -> List[str]:
        """
        Get the names of all ontologies in the OntoEnv.

        Returns:
            A list of ontology names.
        """
        ...

    def to_rdflib_dataset(self) -> Dataset:
        """
        Convert the OntoEnv to an rdflib.Dataset.
        """
        ...

    # Config accessors
    def is_offline(self) -> bool:
        """
        Checks if the environment is in offline mode.
        """
        ...

    def set_offline(self, offline: bool) -> None:
        """
        Sets the offline mode for the environment.
        """
        ...

    def is_strict(self) -> bool:
        """
        Checks if the environment is in strict mode.
        """
        ...

    def set_strict(self, strict: bool) -> None:
        """
        Sets the strict mode for the environment.
        """
        ...

    def requires_ontology_names(self) -> bool:
        """
        Checks if the environment requires unique ontology names.
        """
        ...

    def set_require_ontology_names(self, require: bool) -> None:
        """
        Sets whether the environment requires unique ontology names.
        """
        ...

    def no_search(self) -> bool:
        """
        Checks if the environment disables local file search.
        """
        ...

    def set_no_search(self, no_search: bool) -> None:
        """
        Sets whether the environment disables local file search.
        """
        ...

    def resolution_policy(self) -> str:
        """
        Returns the current resolution policy.
        """
        ...

    def set_resolution_policy(self, policy: str) -> None:
        """
        Sets the resolution policy for the environment.
        """
        ...

    def store_path(self) -> Optional[str]:
        """
        Returns the path to the underlying graph store, if applicable.
        """
        ...

    def close(self) -> None:
        """
        Closes the ontology environment, saving changes and flushing the store.
        """
        ...

    def flush(self) -> None:
        """
        Flushes any pending writes to the underlying graph store.
        """
        ...
