use anyhow::Result;
use clap::{Parser, Subcommand};
use ontoenv::ontology::OntologyLocation;
use ontoenv::util::write_dataset_to_file;
use ontoenv::{config::Config, OntoEnv};
use oxigraph::model::NamedNode;
use std::env::current_dir;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "ontoenv")]
#[command(about = "Ontology environment manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a new ontology environment
    Init {
        search_directories: Vec<PathBuf>,
        #[clap(long, short, action)]
        require_ontology_names: bool,
        #[clap(long, short, num_args = 1..)]
        includes: Vec<String>,
        #[clap(long, short, num_args = 1..)]
        excludes: Vec<String>,
    },
    /// Update the ontology environment
    Refresh,
    /// Compute the owl:imports closure of an ontology and write it to a file
    GetClosure {
        ontology: String,
        destination: Option<String>,
    },
    /// Add an ontology to the environment
    Add {
        #[clap(long, short)]
        url: Option<String>,
        #[clap(long, short)]
        file: Option<String>,
    },
    // TODO: dump all ontologies; nest by ontology name (sorted), w/n each ontology name list all
    // the places where that graph can be found. List basic stats: the metadata field in the
    // Ontology struct and # of triples in the graph; last updated; etc
    /// Print out the current state of the ontology environment
    Dump,
    /// Generate a PDF of the dependency graph
    DepGraph {
        destination: Option<String>,
    },
}

fn main() -> Result<()> {
    env_logger::init();

    let cmd = Cli::parse();

    match cmd.command {
        Commands::Init {
            search_directories,
            require_ontology_names,
            includes,
            excludes,
        } => {
            let config = Config::new(
                current_dir()?,
                search_directories,
                &includes,
                &excludes,
                require_ontology_names,
            )?;
            let mut env = OntoEnv::new(config)?;
            env.update()?;
            env.save_to_directory()?;
        }
        Commands::Refresh => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let mut env = OntoEnv::from_file(&path)?;
            env.update()?;
            env.save_to_directory()?;
        }
        Commands::GetClosure {
            ontology,
            destination,
        } => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            // if the path doesn't exist, raise an error
            if !path.exists() {
                return Err(anyhow::anyhow!(
                    "OntoEnv not found. Run `ontoenv init` to create a new OntoEnv."
                ));
            }
            let env = OntoEnv::from_file(&path)?;

            // make ontology an IRI
            let iri = NamedNode::new(ontology).map_err(|e| anyhow::anyhow!(e.to_string()))?;

            let ont = env
                .get_ontology_by_name(iri.as_ref())
                .ok_or(anyhow::anyhow!("Ontology not found"))?;
            let closure = env.get_dependency_closure(ont.id())?;
            let graph = env.get_union_graph(&closure)?;
            // write the graph to a file
            if let Some(destination) = destination {
                write_dataset_to_file(&graph, &destination)?;
            } else {
                write_dataset_to_file(&graph, "output.ttl")?;
            }
        }
        Commands::Add { url, file } => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let mut env = OntoEnv::from_file(&path)?;

            let location: OntologyLocation = match (url, file) {
                (Some(url), None) => OntologyLocation::Url(url),
                (None, Some(file)) => OntologyLocation::File(PathBuf::from(file)),
                _ => return Err(anyhow::anyhow!("Must specify either --url or --file")),
            };

            env.add(location)?;
        }
        Commands::Dump => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            env.dump();
        }
        Commands::DepGraph { destination } => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            let dot = env.dep_graph_to_dot()?;
            // call graphviz to generate PDF
            let dot_path = current_dir()?.join("dep_graph.dot");
            std::fs::write(&dot_path, dot)?;
            let output = std::process::Command::new("dot")
                .arg("-Tpdf")
                .arg(dot_path)
                .arg("-o")
                .arg(destination.unwrap_or("dep_graph.pdf".to_string()))
                .output()?;
        }
    }

    Ok(())
}
