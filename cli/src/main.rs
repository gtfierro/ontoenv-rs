use anyhow::Result;
use clap::{Parser, Subcommand};
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::util::write_dataset_to_file;
use ontoenv::{config::Config, OntoEnv};
use oxigraph::model::{NamedNode, NamedNodeRef};
use std::env::current_dir;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "ontoenv")]
#[command(about = "Ontology environment manager")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Verbose mode - sets the RUST_LOG level to info, defaults to warning level
    #[clap(long, short, action)]
    verbose: Option<String>,
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
    /// List the ontologies in the environment sorted by name
    ListOntologies,
    /// List the locations of the ontologies in the environment sorted by location
    ListLocations,
    // TODO: dump all ontologies; nest by ontology name (sorted), w/n each ontology name list all
    // the places where that graph can be found. List basic stats: the metadata field in the
    // Ontology struct and # of triples in the graph; last updated; etc
    /// Print out the current state of the ontology environment
    Dump,
    /// Generate a PDF of the dependency graph
    DepGraph {
        roots: Option<Vec<String>>,
        #[clap(long, short)]
        output: Option<String>,
    },
}

fn main() -> Result<()> {
    let cmd = Cli::parse();

    let log_level = cmd.verbose.unwrap_or_else(|| "warning".to_string());
    std::env::set_var("RUST_LOG", log_level);
    env_logger::init();

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
        Commands::ListOntologies => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            // print list of ontology URLs from env.onologies.values() sorted alphabetically
            let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
            ontologies.sort_by(|a, b| a.name().cmp(&b.name()));
            for ont in ontologies {
                println!("{}", ont.name());
            }
        }
        Commands::ListLocations => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
            ontologies.sort_by(|a, b| a.location().as_str().cmp(&b.location().as_str()));
            for ont in ontologies {
                println!("{}", ont.location().as_str());
            }
        }
        Commands::Dump => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            env.dump();
        }
        Commands::DepGraph { roots, output } => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            let dot = if let Some(roots) = roots {
                let roots: Vec<GraphIdentifier> = roots
                    .iter()
                    .map(|iri| env.get_ontology_by_name(NamedNodeRef::new(iri).unwrap()).unwrap().id().clone())
                    .collect();
                env.rooted_dep_graph_to_dot(roots)?
            } else {
                env.dep_graph_to_dot()?
            };
            // call graphviz to generate PDF
            let dot_path = current_dir()?.join("dep_graph.dot");
            std::fs::write(&dot_path, dot)?;
            let output_path = output.unwrap_or_else(|| "dep_graph.pdf".to_string());
            let output = std::process::Command::new("dot")
                .args(&["-Tpdf", dot_path.to_str().unwrap(), "-o", &output_path])
                .output()?;
            if !output.status.success() {
                return Err(anyhow::anyhow!(
                    "Failed to generate PDF: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
    }

    Ok(())
}
