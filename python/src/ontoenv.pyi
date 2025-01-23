from typing import Optional, List, Union

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
        path: Optional[Union[str, Path]] = ".",
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

    def is_read_only(self) -> bool:
        """
        Check if the ontology environment is read-only.

        Returns:
            A boolean indicating if the environment is read-only.
        """
        ...

    def __repr__(self) -> str:
        """
        Return a string representation of the OntoEnv object.
        """
        ...

    def import_graph(self, destination_graph, uri: str) -> None:
        """
        Import a graph from the given URI into the destination graph.

        Args:
            destination_graph: The graph to import into.
            uri: The URI of the graph to import.
        """
        ...

    def list_closure(self, uri: str) -> List[str]:
        """
        List the ontologies in the imports closure of the given ontology.

        Args:
            uri: The URI of the ontology.

        Returns:
            A list of ontology names in the closure.
        """
        ...

    def get_closure(
        self,
        uri: str,
        destination_graph: Optional = None,
        rewrite_sh_prefixes: bool = False,
        remove_owl_imports: bool = False,
    ) -> None:
        """
        Merge all graphs in the imports closure of the given ontology into a single graph.

        Args:
            uri: The URI of the ontology.
            destination_graph: Optional graph to add the merged graph to.
            rewrite_sh_prefixes: Flag to rewrite SH prefixes.
            remove_owl_imports: Flag to remove OWL imports.
        """
        ...

    def dump(self, includes: Optional[str] = None) -> None:
        """
        Print the contents of the OntoEnv.

        Args:
            includes: Optional string to filter the output.
        """
        ...

    def import_dependencies(self, graph) -> None:
        """
        Import the dependencies of the given graph into the graph.

        Args:
            graph: The graph to import dependencies into.
        """
        ...

    def add(self, location) -> None:
        """
        Add a new ontology to the OntoEnv.

        Args:
            location: The location of the ontology to add.
        """
        ...

    def refresh(self) -> None:
        """
        Refresh the OntoEnv by re-loading all remote graphs and loading any local graphs which have changed.
        """
        ...

    def get_graph(self, uri: str) -> None:
        """
        Export the graph with the given URI to an rdflib.Graph.

        Args:
            uri: The URI of the graph to export.
        """
        ...

    def get_ontology_names(self) -> List[str]:
        """
        Get the names of all ontologies in the OntoEnv.

        Returns:
            A list of ontology names.
        """
        ...

    def to_rdflib_dataset(self) -> None:
        """
        Convert the OntoEnv to an rdflib.Dataset.
        """
        ...
