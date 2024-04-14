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
    #[clap(long, short, action, default_value = "false")]
    verbose: bool,
    /// Resolution policy for determining which ontology to use when there are multiple with the same name
    #[clap(long, short, default_value = "default")]
    policy: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a new ontology environment
    Init {
        /// Directories to search for ontologies. If not provided, the current directory is used.
        search_directories: Option<Vec<PathBuf>>,
        /// Require ontology names to be unique; will raise an error if multiple ontologies have the same name
        #[clap(long, short, action)]
        require_ontology_names: bool,
        /// Strict mode - will raise an error if an ontology is not found
        #[clap(long, short, action, default_value = "false")]
        strict: bool,
        /// Offline mode - will not attempt to fetch ontologies from the web
        #[clap(long, short, action, default_value = "false")]
        offline: bool,
        /// Glob patterns for which files to include, defaults to ['*.ttl','*.xml','*.n3']
        #[clap(long, short, num_args = 1..)]
        includes: Vec<String>,
        /// Glob patterns for which files to exclude, defaults to []
        #[clap(long, short, num_args = 1..)]
        excludes: Vec<String>,
    },
    /// Update the ontology environment
    Refresh,
    /// Compute the owl:imports closure of an ontology and write it to a file
    GetClosure {
        /// The name (URI) of the ontology to compute the closure for
        ontology: String,
        /// Rewrite the sh:prefixes declarations to point to the chosen ontology, defaults to true
        #[clap(long, short, action, default_value = "true")]
        rewrite_sh_prefixes: Option<bool>,
        /// Remove owl:imports statements from the closure, defaults to true
        #[clap(long, short, action, default_value = "true")]
        remove_owl_imports: Option<bool>,
        /// The file to write the closure to, defaults to 'output.ttl'
        destination: Option<String>,
    },
    /// Add an ontology to the environment
    Add {
        /// The URL of the ontology to add
        #[clap(long, short)]
        url: Option<String>,
        /// The path to the file to add
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
        /// The root ontologies to start the graph from. Given by name (URI)
        roots: Option<Vec<String>>,
        /// The output file to write the PDF to, defaults to 'dep_graph.pdf'
        #[clap(long, short)]
        output: Option<String>,
    },
    /// Run the doctor to check the environment for issues
    Doctor,
}

fn main() -> Result<()> {
    let cmd = Cli::parse();

    let log_level = if cmd.verbose { "info" } else { "warn" };
    std::env::set_var("RUST_LOG", log_level);
    env_logger::init();

    let policy = cmd.policy.unwrap_or_else(|| "default".to_string());

    match cmd.command {
        Commands::Init {
            search_directories,
            require_ontology_names,
            strict,
            offline,
            includes,
            excludes,
        } => {
            // if search_directories is empty, use the current directory
            let config = Config::new(
                current_dir()?,
                search_directories,
                &includes,
                &excludes,
                require_ontology_names,
                strict,
                offline,
                policy,
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
            rewrite_sh_prefixes,
            remove_owl_imports,
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
            let graph = env.get_union_graph(&closure, rewrite_sh_prefixes, remove_owl_imports)?;
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
            ontologies.dedup_by(|a, b| a.name() == b.name());
            for ont in ontologies {
                println!("{}", ont.name().as_str());
            }
        }
        Commands::ListLocations => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            let mut ontologies: Vec<&GraphIdentifier> = env.ontologies().keys().collect();
            ontologies.sort_by(|a, b| a.location().as_str().cmp(b.location().as_str()));
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
                    .map(|iri| {
                        env.get_ontology_by_name(NamedNodeRef::new(iri).unwrap())
                            .unwrap()
                            .id()
                            .clone()
                    })
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
                .args(["-Tpdf", dot_path.to_str().unwrap(), "-o", &output_path])
                .output()?;
            if !output.status.success() {
                return Err(anyhow::anyhow!(
                    "Failed to generate PDF: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        Commands::Doctor => {
            // load env from .ontoenv/ontoenv.json
            let path = current_dir()?.join(".ontoenv/ontoenv.json");
            let env = OntoEnv::from_file(&path)?;
            env.doctor();
        }
    }

    Ok(())
}
