from typing import Optional, List, Union

class Config:
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
    ) -> None: ...

class OntoEnv:
    def __init__(
        self,
        config: Optional[Config] = None,
        path: Optional[Union[str, Path]] = ".",
        recreate: bool = False,
        read_only: bool = False,
    ) -> None: ...

    def update(self) -> None: ...

    def is_read_only(self) -> bool: ...

    def __repr__(self) -> str: ...

    def import_graph(self, destination_graph, uri: str) -> None: ...

    def list_closure(self, uri: str) -> List[str]: ...

    def get_closure(
        self,
        uri: str,
        destination_graph: Optional = None,
        rewrite_sh_prefixes: bool = False,
        remove_owl_imports: bool = False,
    ) -> None: ...

    def dump(self, includes: Optional[str] = None) -> None: ...

    def import_dependencies(self, graph) -> None: ...

    def add(self, location) -> None: ...

    def refresh(self) -> None: ...

    def get_graph(self, uri: str) -> None: ...

    def get_ontology_names(self) -> List[str]: ...

    def to_rdflib_dataset(self) -> None: ...
